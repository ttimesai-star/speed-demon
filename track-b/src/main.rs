// Speed Demon 2026, track B (Index). Spec: specs/b-index.md in hackathon-raptors/speed-demon-2026.
use std::io::Write;
use std::sync::atomic::{AtomicUsize, Ordering::Relaxed};
use std::sync::OnceLock;
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

const NONE: u32 = u32::MAX;

// Runs f(0..nt) on nt threads and returns the results in thread order.
fn par<T: Send>(nt: usize, f: impl Fn(usize) -> T + Sync) -> Vec<T> {
    thread::scope(|sc| {
        let hs: Vec<_> = (0..nt).map(|t| sc.spawn({ let f = &f; move || f(t) })).collect();
        hs.into_iter().map(|h| h.join().unwrap()).collect()
    })
}

// Eight bytes at d[p..], little-endian, zero-padded past the end of d.
fn word(d: &[u8], p: usize) -> u64 {
    if p + 8 <= d.len() {
        return u64::from_le(unsafe { (d.as_ptr().add(p) as *const u64).read_unaligned() });
    }
    let mut b = [0u8; 8];
    if p < d.len() {
        b[..d.len() - p].copy_from_slice(&d[p..]);
    }
    u64::from_le_bytes(b)
}

const ONES: u64 = 0x0101_0101_0101_0101;
const HI: u64 = 0x8080_8080_8080_8080;

// High bit of each byte of w in [a, b]; bytes of 0x80 and above never match.
fn range(w: u64, a: u8, b: u8) -> u64 {
    let x = w & !HI;
    (x + (0x80 - a as u64) * ONES) & !(x + (0x7F - b as u64) * ONES) & !w & HI
}

fn lower8(w: u64) -> u64 {
    w | range(w, b'A', b'Z') >> 2
}

// High bit of each byte that belongs to a token (a-z, 0-9 after ASCII-lowercasing).
fn tokmask(w: u64) -> u64 {
    let l = lower8(w);
    range(l, b'a', b'z') | range(l, b'0', b'9')
}

// The lowercased k-th word of the token d[s..s+n], bytes past n zeroed.
fn tword(d: &[u8], s: usize, n: usize, k: usize) -> u64 {
    lower8(word(d, s + 8 * k)) & u64::MAX >> 8 * (8 - (n - 8 * k).min(8)) as u32 % 64
}

fn thash(d: &[u8], s: usize, n: usize) -> u64 {
    (0..n.div_ceil(8)).fold(n as u64, |h, k| (h ^ tword(d, s, n, k)).wrapping_mul(0x9E37_79B9_7F4A_7C15))
}

// The query terms: open addressing over hash tag << 32 | (id + 1); the bytes are checked against queries.txt.
struct Dict<'a> {
    q: &'a [u8],
    terms: Vec<(usize, usize)>,
    slots: Vec<u64>,
}

impl Dict<'_> {
    fn new(q: &[u8], terms: Vec<(usize, usize)>) -> Dict<'_> {
        let mut slots = vec![0u64; (4 * terms.len()).next_power_of_two().max(16)];
        for (id, &(o, n)) in terms.iter().enumerate() {
            let h = thash(q, o, n);
            let mut i = (h >> 40) as usize & (slots.len() - 1);
            while slots[i] != 0 {
                i = (i + 1) & (slots.len() - 1);
            }
            slots[i] = (h >> 8) << 32 | (id as u64 + 1);
        }
        Dict { q, terms, slots }
    }
    // Id of the token d[s..s+n] (lowercased), or NONE.
    fn get(&self, d: &[u8], s: usize, n: usize) -> u32 {
        let h = thash(d, s, n);
        let m = self.slots.len() - 1;
        let mut i = (h >> 40) as usize & m;
        loop {
            let e = self.slots[i];
            if e == 0 {
                return NONE;
            }
            if e >> 32 == (h >> 8) & 0xFFFF_FFFF {
                let (o, l) = self.terms[(e as u32 - 1) as usize];
                if l == n && (0..n.div_ceil(8)).all(|k| tword(d, s, n, k) == tword(self.q, o, n, k)) {
                    return e as u32 - 1;
                }
            }
            i = (i + 1) & m;
        }
    }
}

// Calls f(term id) for each token of text, in order (NONE for a token that is no query term).
fn tokens(dict: &Dict, text: &[u8], mut f: impl FnMut(u32)) {
    let mut p = 0;
    while p < text.len() {
        let m = tokmask(word(text, p));
        if m == 0 {
            p += 8;
            continue;
        }
        p += m.trailing_zeros() as usize / 8;
        let mut n = 0;
        loop {
            let z = !tokmask(word(text, p + n)) & HI;
            if z != 0 {
                n += z.trailing_zeros() as usize / 8;
                break;
            }
            n += 8;
        }
        f(if n > 32 { NONE } else { dict.get(text, p, n) });
        p += n;
    }
}

// First index i >= j with b[i] >= x, galloping from j (b sorted).
fn seek(b: &[u32], mut j: usize, x: u32) -> usize {
    let mut step = 1;
    while j + step < b.len() && b[j + step] < x {
        (j, step) = (j + step, step * 2);
    }
    let hi = (j + step).min(b.len());
    j + b[j.min(hi)..hi].partition_point(|&y| y < x)
}

fn and(a: &[u32], b: &[u32]) -> Vec<u32> {
    let (a, b) = if a.len() <= b.len() { (a, b) } else { (b, a) };
    let (mut r, mut j) = (Vec::with_capacity(a.len()), 0);
    for &x in a {
        j = seek(b, j, x);
        if j < b.len() && b[j] == x {
            r.push(x);
        }
    }
    r
}

fn or(a: &[u32], b: &[u32]) -> Vec<u32> {
    let (mut r, mut i, mut j) = (Vec::with_capacity(a.len() + b.len()), 0, 0);
    while i < a.len() && j < b.len() {
        let x = a[i].min(b[j]);
        (i += (a[i] == x) as usize, j += (b[j] == x) as usize);
        r.push(x);
    }
    r.extend_from_slice(&a[i..]);
    r.extend_from_slice(&b[j..]);
    r
}

fn not(a: &[u32], b: &[u32]) -> Vec<u32> {
    let (mut r, mut j) = (Vec::with_capacity(a.len()), 0);
    for &x in a {
        j = seek(b, j, x);
        if j == b.len() || b[j] != x {
            r.push(x);
        }
    }
    r
}

enum Query {
    Bool(Vec<(u8, u32)>), // (operator byte: b'A', b'O', b'N', or 0 for the first term; term id)
    Phrase(Vec<u32>),
}

fn main() {
    let arg = |i: usize, def: &str| env::args().nth(i).unwrap_or(def.into());
    let (din, dout) = (arg(1, "/data"), arg(2, "/out"));
    let q = fs::read(format!("{din}/queries.txt")).unwrap();
    // Queries and the dictionary of their terms.
    let mut terms: Vec<(usize, usize)> = vec![]; // (offset, length) of each distinct term
    let mut ids: std::collections::HashMap<&[u8], u32> = Default::default();
    let mut id_of = |o: usize, n: usize| -> u32 {
        *ids.entry(&q[o..o + n]).or_insert_with(|| (terms.push((o, n)), terms.len() as u32 - 1).1)
    };
    let mut qs = vec![];
    for line in q.split(|&c| c == b'\n').filter(|l| !l.is_empty()) {
        let o = line.as_ptr() as usize - q.as_ptr() as usize;
        let mut words = vec![];
        let mut p = 0;
        for w in line.split(|&c| c == b' ') {
            words.push((o + p, w.len()));
            p += w.len() + 1;
        }
        if line.starts_with(b"BOOL") {
            let mut v = vec![(0, id_of(words[1].0, words[1].1))];
            for k in (2..words.len()).step_by(2) {
                v.push((q[words[k].0], id_of(words[k + 1].0, words[k + 1].1)));
            }
            qs.push(Query::Bool(v));
        } else {
            let last = words.len() - 1;
            (words[1] = (words[1].0 + 1, words[1].1 - 1), words[last].1 -= 1); // the quotes
            qs.push(Query::Phrase(words[1..].iter().map(|&(o, n)| id_of(o, n)).collect()));
        }
    }
    let nterms = terms.len();
    let dict = Dict::new(&q, terms);
    // A term that a phrase repeats k times in a row: each document's longest run of it is recorded.
    let mut rep = vec![NONE; nterms]; // index into the run lists
    let mut nrep = 0;
    for x in &qs {
        if let Query::Phrase(v) = x {
            for k in 1..v.len() {
                if v[k] == v[k - 1] && rep[v[k] as usize] == NONE {
                    (rep[v[k] as usize] = nrep, nrep += 1);
                }
            }
        }
    }
    // Corpus: one chunk per thread at line starts; per thread a posting list per term.
    let d = load(&format!("{din}/docs.txt"));
    let nt = thread::available_parallelism().map_or(12, |n| n.get());
    let mut bs: Vec<usize> = (0..nt).map(|k| k * d.len() / nt).collect();
    for b in &mut bs[1..] {
        while *b < d.len() && d[*b - 1] != b'\n' {
            *b += 1;
        }
    }
    bs.push(d.len());
    let (dict, rep, bs) = (&dict, &rep, &bs);
    let parts = par(nt, |t| {
        let (mut lists, mut last) = (vec![Vec::<u32>::new(); nterms], vec![NONE; nterms]);
        let (mut runs, mut docs) = (vec![Vec::<(u32, u32)>::new(); nrep as usize], vec![]);
        let mut p = bs[t];
        while p < bs[t + 1] {
            let tab = p + d[p..].iter().position(|&c| c == b'\t').unwrap();
            let id = d[p..tab].iter().fold(0u32, |x, &c| x * 10 + (c - b'0') as u32);
            let end = tab + d[tab..].iter().position(|&c| c == b'\n').unwrap();
            docs.push((tab as u32 + 1, end as u32));
            let (mut prev, mut run) = (NONE, 0);
            tokens(dict, &d[tab + 1..end], |w| {
                run = if w == prev { run + 1 } else { 1 };
                prev = w;
                if w == NONE {
                    return;
                }
                if last[w as usize] != id {
                    (last[w as usize] = id, lists[w as usize].push(id));
                }
                let r = rep[w as usize];
                if r != NONE && run >= 2 {
                    let l = &mut runs[r as usize];
                    match l.last_mut() {
                        Some(e) if e.0 == id => e.1 = e.1.max(run),
                        _ => l.push((id, run)),
                    }
                }
            });
            p = end + 1;
        }
        (lists, runs, docs)
    });
    let mut lists: Vec<Vec<u32>> = vec![vec![]; nterms];
    let mut runs: Vec<Vec<(u32, u32)>> = vec![vec![]; nrep as usize];
    let mut docs = vec![];
    for (l, r, dd) in parts {
        for (a, b) in lists.iter_mut().zip(l) {
            if a.is_empty() { *a = b } else { a.extend_from_slice(&b) }
        }
        for (a, b) in runs.iter_mut().zip(r) {
            a.extend_from_slice(&b);
        }
        docs.extend(dd);
    }
    // Queries on all threads, answers by index.
    let ans: Vec<OnceLock<Vec<u8>>> = (0..qs.len()).map(|_| OnceLock::new()).collect();
    let next = AtomicUsize::new(0);
    let (lists, runs, docs, qs, ans, next) = (&lists, &runs, &docs, &qs, &ans, &next);
    par(nt, |_| loop {
        let i = next.fetch_add(1, Relaxed);
        let Some(x) = qs.get(i) else { break };
        let r: Vec<u32> = match x {
            Query::Bool(v) => {
                let mut r = lists[v[0].1 as usize].clone();
                for &(op, w) in &v[1..] {
                    let b = &lists[w as usize];
                    r = match op {
                        b'A' => and(&r, b),
                        b'O' => or(&r, b),
                        _ => not(&r, b),
                    };
                }
                r
            }
            Query::Phrase(v) => {
                // Candidates: every term's list, and the run list of each term repeated k times in a row.
                let mut sets: Vec<&[u32]> = v.iter().map(|&w| &lists[w as usize][..]).collect();
                sets.sort_by_key(|s| s.len());
                let mut c = sets[0].to_vec();
                for s in &sets[1..] {
                    c = and(&c, s);
                }
                let (mut k, mut need) = (0, vec![]);
                while k < v.len() {
                    let mut e = k;
                    while e + 1 < v.len() && v[e + 1] == v[k] {
                        e += 1;
                    }
                    if e > k {
                        need.push((rep[v[k] as usize], e - k + 1));
                    }
                    k = e + 1;
                }
                for (r, len) in need {
                    let l: Vec<u32> = runs[r as usize].iter().filter(|e| e.1 as usize >= len).map(|e| e.0).collect();
                    c = and(&c, &l);
                }
                c.retain(|&doc| {
                    let (s, e) = docs[doc as usize];
                    let mut seq = vec![];
                    tokens(dict, &d[s as usize..e as usize], |w| seq.push(w));
                    seq.windows(v.len()).any(|win| win == &v[..])
                });
                c
            }
        };
        let mut o = Vec::new();
        write!(o, "{} {}", r.len(), r.iter().map(|&x| x as u64).sum::<u64>()).unwrap();
        for x in r.iter().take(10) {
            write!(o, " {x}").unwrap();
        }
        o.push(b'\n');
        ans[i].set(o).unwrap();
    });
    let mut out = Vec::new();
    for a in ans {
        out.extend_from_slice(a.get().unwrap());
    }
    fs::create_dir_all(&dout).unwrap();
    fs::write(format!("{dout}/results.txt"), out).unwrap();
}
