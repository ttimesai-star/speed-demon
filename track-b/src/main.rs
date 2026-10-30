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

fn lower(c: u8) -> u8 {
    c | ((c.wrapping_sub(b'A') < 26) as u8) << 5
}

fn is_tok(c: u8) -> bool {
    c.wrapping_sub(b'a') < 26 || c.wrapping_sub(b'0') < 10
}

fn hash(h: u64, c: u8) -> u64 {
    (h ^ c as u64).wrapping_mul(0x100_0000_01b3)
}

// The query terms: open addressing over (offset in queries.txt + 1) << 8 | length, with their ids.
struct Dict<'a> {
    q: &'a [u8],
    slots: Vec<(u64, u32)>,
}

impl Dict<'_> {
    // Id of the lowercased token whose bytes are d[s..s+n] and whose hash is h, or NONE.
    fn get(&self, d: &[u8], s: usize, n: usize, h: u64) -> u32 {
        let m = self.slots.len() - 1;
        let mut i = (h >> 32) as usize & m;
        loop {
            let (k, id) = self.slots[i];
            if k == 0 {
                return NONE;
            }
            let (o, l) = ((k >> 8) as usize - 1, (k & 255) as usize);
            if l == n && (0..n).all(|j| lower(d[s + j]) == self.q[o + j]) {
                return id;
            }
            i = (i + 1) & m;
        }
    }
}

// Calls f(term id) for each token of text, in order (NONE for a token that is no query term).
fn tokens(dict: &Dict, text: &[u8], mut f: impl FnMut(u32)) {
    let mut p = 0;
    while p < text.len() {
        if !is_tok(lower(text[p])) {
            p += 1;
            continue;
        }
        let (s, mut h) = (p, 0xcbf2_9ce4_8422_2325u64);
        while p < text.len() && is_tok(lower(text[p])) {
            h = hash(h, lower(text[p]));
            p += 1;
        }
        f(if p - s > 32 { NONE } else { dict.get(text, s, p - s, h) });
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
    let mut slots = vec![(0u64, 0u32); 1 << 20];
    let mut id_of = |o: usize, n: usize| -> u32 {
        let h = q[o..o + n].iter().fold(0xcbf2_9ce4_8422_2325u64, |h, &c| hash(h, c));
        let dict = Dict { q: &q, slots: std::mem::take(&mut slots) };
        let id = dict.get(&q, o, n, h);
        slots = dict.slots;
        if id != NONE {
            return id;
        }
        let mut i = (h >> 32) as usize & (slots.len() - 1);
        while slots[i].0 != 0 {
            i = (i + 1) & (slots.len() - 1);
        }
        slots[i] = (((o + 1) as u64) << 8 | n as u64, terms.len() as u32);
        terms.push((o, n));
        terms.len() as u32 - 1
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
    let dict = Dict { q: &q, slots };
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
