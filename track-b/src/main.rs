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

#[derive(Clone)]
enum Set {
    List(Vec<u32>), // sorted document ids
    Bits(Vec<u64>), // bit i = document base + i
}

// A thread's set of one term, taken by the one merge worker that owns the term.
struct Cell(*mut Set);
unsafe impl Send for Cell {}
unsafe impl Sync for Cell {}

// Document ids of s in ascending order (bit i of a bitset is document base + i).
fn members(s: &Set, base: usize) -> Box<dyn Iterator<Item = usize> + '_> {
    match s {
        Set::List(l) => Box::new(l.iter().map(|&x| x as usize)),
        Set::Bits(b) => Box::new(b.iter().enumerate().flat_map(move |(k, &w)| {
            let mut w = w;
            std::iter::from_fn(move || (w != 0).then(|| (base + 64 * k + w.trailing_zeros() as usize, w &= w - 1).0))
        })),
    }
}

// The list s plus document base + i, as a bitset of `range` documents from base.
fn bits(s: &Set, base: usize, range: usize, i: usize) -> Vec<u64> {
    let mut b = vec![0u64; range / 64 + 1];
    for j in members(s, 0).map(|x| x - base).chain([i]) {
        b[j / 64] |= 1 << (j % 64);
    }
    b
}

fn has(b: &[u64], x: u32) -> bool {
    b[x as usize / 64] >> (x % 64) & 1 != 0
}

// Bit planes: popcount(w & PLANES[j]) << j summed over j is the sum of the set bit positions of w.
const PLANES: [u64; 6] = [
    0xAAAA_AAAA_AAAA_AAAA, 0xCCCC_CCCC_CCCC_CCCC, 0xF0F0_F0F0_F0F0_F0F0,
    0xFF00_FF00_FF00_FF00, 0xFFFF_0000_FFFF_0000, 0xFFFF_FFFF_0000_0000,
];

// r AND / OR / NOT s, for every mix of lists and bitsets of all documents.
fn apply(r: Set, op: u8, s: &Set) -> Set {
    use Set::*;
    match (op, r, s) {
        (b'A', List(a), List(c)) => List(and(&a, c)),
        (b'A', List(a), Bits(c)) => List(a.into_iter().filter(|&x| has(c, x)).collect()),
        (b'A', Bits(a), List(c)) => List(c.iter().copied().filter(|&x| has(&a, x)).collect()),
        (b'A', Bits(mut a), Bits(c)) => (a.iter_mut().zip(c).for_each(|(x, y)| *x &= y), Bits(a)).1,
        (b'O', List(a), List(c)) => List(or(&a, c)),
        (b'O', List(a), Bits(c)) => Bits(c.clone()).or_list(&a),
        (b'O', Bits(a), List(c)) => Bits(a).or_list(c),
        (b'O', Bits(mut a), Bits(c)) => (a.iter_mut().zip(c).for_each(|(x, y)| *x |= y), Bits(a)).1,
        (_, List(a), List(c)) => List(not(&a, c)),
        (_, List(a), Bits(c)) => List(a.into_iter().filter(|&x| !has(c, x)).collect()),
        (_, Bits(mut a), List(c)) => (c.iter().for_each(|&x| a[x as usize / 64] &= !(1 << (x % 64))), Bits(a)).1,
        (_, Bits(mut a), Bits(c)) => (a.iter_mut().zip(c).for_each(|(x, y)| *x &= !y), Bits(a)).1,
    }
}

impl Set {
    fn or_list(self, l: &[u32]) -> Set {
        let Set::Bits(mut b) = self else { unreachable!() };
        l.iter().for_each(|&x| b[x as usize / 64] |= 1 << (x % 64));
        Set::Bits(b)
    }
}

enum Query {
    Bool(Vec<(u8, u32)>), // (operator byte: b'A', b'O', b'N', or 0 for the first term; term id)
    Phrase(usize), // index into the phrase list
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
    let (mut qs, mut phr) = (vec![], vec![]);
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
            phr.push(words[1..].iter().map(|&(o, n)| id_of(o, n)).collect::<Vec<u32>>());
            qs.push(Query::Phrase(phr.len() - 1));
        }
    }
    let nterms = terms.len();
    let dict = Dict::new(&q, terms);
    // Each phrase is checked in the corpus pass at the occurrences of one trigger term, its longest term
    // (long words are the rare ones; any choice is correct, a rare one is cheaper).
    let mut trig: Vec<Vec<(u32, u32)>> = vec![vec![]; nterms]; // term -> (phrase, offset in the phrase)
    for (k, v) in phr.iter().enumerate() {
        let j = (0..v.len()).max_by_key(|&j| (dict.terms[v[j] as usize].1, usize::MAX - j)).unwrap();
        trig[v[j] as usize].push((k as u32, j as u32));
    }
    // Corpus: one chunk per thread at line starts. Thread t holds the documents [first[t], first[t + 1]);
    // its posting list of a term becomes a bitset of that range once it is longer than the range / 32.
    let d = load(&format!("{din}/docs.txt"));
    let nt = 12;
    let mut bs: Vec<usize> = (0..nt).map(|k| k * d.len() / nt).collect();
    for b in &mut bs[1..] {
        while *b < d.len() && d[*b - 1] != b'\n' {
            *b += 1;
        }
    }
    bs.push(d.len());
    let id_at = |p: usize| d[p..].iter().take_while(|c| c.is_ascii_digit()).fold(0, |x, &c| x * 10 + (c - b'0') as usize);
    let last_line = d[..d.len().saturating_sub(1)].iter().rposition(|&c| c == b'\n').map_or(0, |x| x + 1);
    let n = if d.is_empty() { 0 } else { id_at(last_line) + 1 };
    let first: Vec<usize> = (0..=nt).map(|t| if bs[t] < d.len() { id_at(bs[t]) } else { n }).collect();
    let (dict, trig, phr, bs, first) = (&dict, &trig, &phr, &bs, &first);
    let mut parts = par(nt, |t| {
        let range = first[t + 1] - first[t];
        let mut sets: Vec<Set> = (0..nterms).map(|_| Set::List(vec![])).collect();
        let mut last = vec![NONE; nterms];
        let (mut seq, mut hits, mut found, mut lastm) = (vec![], vec![], vec![], vec![NONE; phr.len()]);
        let mut p = bs[t];
        while p < bs[t + 1] {
            let tab = p + d[p..].iter().position(|&c| c == b'\t').unwrap();
            let id = id_at(p) as u32;
            let end = tab + d[tab..].iter().position(|&c| c == b'\n').unwrap();
            let i = id as usize - first[t];
            tokens(dict, &d[tab + 1..end], |w| {
                if w != NONE {
                    if last[w as usize] != id {
                        last[w as usize] = id;
                        match &mut sets[w as usize] {
                            Set::Bits(b) => b[i / 64] |= 1 << (i % 64),
                            Set::List(l) if l.len() * 32 < range => l.push(id),
                            s => *s = Set::Bits(bits(s, first[t], range, i)),
                        }
                    }
                    if !trig[w as usize].is_empty() {
                        hits.push((seq.len(), w));
                    }
                }
                seq.push(w);
            });
            for &(at, w) in &hits {
                for &(k, j) in &trig[w as usize] {
                    let v = &phr[k as usize];
                    let s = at.wrapping_sub(j as usize);
                    if s <= at && s + v.len() <= seq.len() && seq[s..s + v.len()] == v[..] && lastm[k as usize] != id {
                        (lastm[k as usize] = id, found.push((k, id)));
                    }
                }
            }
            (seq.clear(), hits.clear());
            p = end + 1;
        }
        (sets, found)
    });
    let mut pres: Vec<Vec<u32>> = vec![vec![]; phr.len()];
    for (_, f) in &parts {
        for &(k, id) in f {
            pres[k as usize].push(id); // threads in order, documents in order: sorted
        }
    }
    // One set per term: a bitset of all n documents if any thread made one or it is long, else the
    // concatenated lists. Term ranges in parallel; every thread part is freed as soon as it is used.
    let cells: Vec<Vec<Cell>> = parts.iter_mut().map(|p| p.0.iter_mut().map(|s| Cell(s)).collect()).collect();
    let cells = &cells;
    let sets: Vec<Set> = par(nt, |c| {
        (c * nterms / nt..(c + 1) * nterms / nt)
            .map(|w| {
                let mine: Vec<Set> = (0..nt).map(|t| (cells[t][w].0, Set::List(vec![]))).map(|(p, e)| std::mem::replace(unsafe { &mut *p }, e)).collect();
                let len: usize = mine.iter().map(|s| if let Set::List(l) = s { l.len() } else { 0 }).sum();
                if len * 32 < n && mine.iter().all(|s| matches!(s, Set::List(_))) {
                    let mut l = Vec::with_capacity(len);
                    for s in mine {
                        if let Set::List(x) = s {
                            l.extend_from_slice(&x);
                        }
                    }
                    return Set::List(l);
                }
                let mut b = vec![0u64; n / 64 + 1];
                for (t, s) in mine.into_iter().enumerate() {
                    for i in members(&s, first[t]) {
                        b[i / 64] |= 1 << (i % 64);
                    }
                }
                Set::Bits(b)
            })
            .collect::<Vec<_>>()
    })
    .concat();
    drop(parts);
    // Queries on all threads, answers by index.
    let ans: Vec<OnceLock<Vec<u8>>> = (0..qs.len()).map(|_| OnceLock::new()).collect();
    let next = AtomicUsize::new(0);
    let (sets, pres, qs, ans, next) = (&sets, &pres, &qs, &ans, &next);
    par(nt, |_| loop {
        let i = next.fetch_add(1, Relaxed);
        let Some(x) = qs.get(i) else { break };
        let r: Set = match x {
            Query::Bool(v) => {
                let mut r = sets[v[0].1 as usize].clone();
                for &(op, w) in &v[1..] {
                    r = apply(r, op, &sets[w as usize]);
                }
                r
            }
            Query::Phrase(k) => Set::List(pres[*k].clone()),
        };
        let (count, sum) = match &r {
            Set::List(l) => (l.len() as u64, l.iter().map(|&x| x as u64).sum::<u64>()),
            Set::Bits(b) => b.iter().enumerate().fold((0, 0), |(c, s), (k, &w)| {
                let pos: u64 = PLANES.iter().enumerate().map(|(j, &m)| ((w & m).count_ones() as u64) << j).sum();
                (c + w.count_ones() as u64, s + 64 * k as u64 * w.count_ones() as u64 + pos)
            }),
        };
        let mut o = Vec::new();
        write!(o, "{count} {sum}").unwrap();
        for x in members(&r, 0).take(10) {
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
