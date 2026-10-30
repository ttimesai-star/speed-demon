// Speed Demon 2026, track B (Index). Spec: specs/b-index.md in hackathon-raptors/speed-demon-2026.
use std::borrow::Cow;
use std::io::Write;
use std::sync::atomic::{AtomicU64, AtomicUsize, Ordering::Relaxed};
use std::sync::OnceLock;
use std::{env, fs, thread};

fn load(path: &str) -> &'static [u8] {
    use std::os::unix::io::AsRawFd;
    extern "C" {
        fn mmap(a: *mut u8, len: usize, prot: i32, flags: i32, fd: i32, off: i64) -> *mut u8;
        fn prctl(op: i32, ...) -> i32;
    }
    unsafe { prctl(41, 1u64, 0u64, 0u64, 0u64) }; // PR_SET_THP_DISABLE: no 2 MB pages under the per-thread heaps
    let f = fs::File::open(path).unwrap();
    let n = f.metadata().unwrap().len() as usize;
    if n == 0 {
        return &[];
    }
    unsafe { std::slice::from_raw_parts(mmap(std::ptr::null_mut(), n, 1, 2, f.as_raw_fd(), 0), n) }
}


const NONE: u32 = u32::MAX;

// Runs f(0..nt) on nt threads and returns the results in thread order.
fn par<T: Send>(nt: usize, f: impl Fn(usize) -> T + Sync) -> Vec<T> {
    let f = &f;
    thread::scope(|sc| {
        let hs: Vec<_> = (0..nt).map(|t| sc.spawn(move || f(t))).collect();
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

// Offset of the first byte c in d[p..], eight bytes at a time (exact zero-byte test).
fn find(d: &[u8], mut p: usize, c: u8) -> usize {
    loop {
        let x = word(d, p) ^ c as u64 * ONES;
        let z = !(((x & !HI) + !HI) | x) & HI;
        if z != 0 {
            return p + z.trailing_zeros() as usize / 8;
        }
        p += 8;
    }
}
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
    // Id of the token d[s..s+n] (lowercased) whose hash is h, or NONE.
    fn get(&self, d: &[u8], s: usize, n: usize, h: u64) -> u32 {
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

// Calls f(term id) for each token of text, in order (NONE for a token that is no query term). One token
// ahead: the next token is found and hashed and its dictionary slot prefetched before this one is resolved.
fn tokens(dict: &Dict, text: &[u8], mut f: impl FnMut(u32)) {
    let mut p = 0;
    let mut next = || loop {
        if p >= text.len() {
            return None;
        }
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
        let h = if n > 32 { 0 } else { thash(text, p, n) };
        let slot = dict.slots.as_ptr().wrapping_add((h >> 40) as usize & (dict.slots.len() - 1));
        unsafe { std::arch::x86_64::_mm_prefetch::<3>(slot as *const i8) };
        p += n;
        return Some((p - n, n, h));
    };
    let mut cur = next();
    while let Some((s, n, h)) = cur {
        cur = next();
        f(if n > 32 { NONE } else { dict.get(text, s, n, h) });
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
    List(Cow<'static, [u32]>), // sorted document ids: a term's list borrows the shared buffer
    Bits(Vec<u64>), // bit i = document base + i
}

impl Set {
    fn bits(self, words: usize) -> Vec<u64> {
        let Set::List(l) = self else { return match self { Set::Bits(b) => b, _ => unreachable!() } };
        let mut b = vec![0; words];
        l.iter().for_each(|&x| b[x as usize / 64] |= 1 << (x % 64));
        b
    }
}

// What one thread's corpus pass leaves: documents per term, its bitsets, and the flat stream of term ids.
struct Part {
    cnt: Vec<u32>,
    bit: Vec<u32>,
    stream: Vec<u32>,
    ends: Vec<u32>,
    tb: Vec<Vec<u64>>,
    found: Vec<(u32, u32)>,
}

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

// r AND / OR / NOT s without copying a bitset: AND keeps the list side, OR and NOT on a bitset work in
// place; only list OR bitset copies the bitset.
fn apply(r: Set, op: u8, s: &Set, words: usize) -> Set {
    use Set::*;
    let has = |b: &[u64], x: u32| b[x as usize / 64] >> (x % 64) & 1 != 0;
    match (op, r, s) {
        (b'A', List(a), List(c)) => List(and(&a, c).into()),
        (b'O', List(a), List(c)) => List(or(&a, c).into()),
        (_, List(a), List(c)) => List(not(&a, c).into()),
        (b'A', List(a), Bits(c)) => List(a.iter().copied().filter(|&x| has(c, x)).collect::<Vec<_>>().into()),
        (b'A', Bits(a), List(c)) => List(c.iter().copied().filter(|&x| has(&a, x)).collect::<Vec<_>>().into()),
        (b'N', List(a), Bits(c)) => List(a.iter().copied().filter(|&x| !has(c, x)).collect::<Vec<_>>().into()),
        (_, r, s) => {
            let mut a = r.bits(words);
            match s {
                Bits(c) => a.iter_mut().zip(c).for_each(|(x, &y)| *x = if op == b'A' { *x & y } else if op == b'O' { *x | y } else { *x & !y }),
                List(c) => c.iter().for_each(|&x| a[x as usize / 64] = if op == b'O' { a[x as usize / 64] | 1 << (x % 64) } else { a[x as usize / 64] & !(1 << (x % 64)) }),
            }
            Bits(a)
        }
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
    let (mut qs, mut phr, mut words) = (vec![], vec![], vec![]);
    for line in q.split(|&c| c == b'\n').filter(|l| !l.is_empty()) {
        let o = line.as_ptr() as usize - q.as_ptr() as usize;
        words.clear();
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
    // Corpus: one chunk per thread at line starts. Thread t holds the documents [first[t], first[t + 1]) and
    // writes, per document, the ids of its distinct query terms to one flat stream; a term that passes
    // range / 32 documents in the thread goes to a bitset of the thread's range instead.
    let d = load(&format!("{din}/docs.txt"));
    let nt = thread::available_parallelism().map_or(12, |n| n.get());
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
    // Each phrase is checked in the corpus pass at the occurrences of one trigger term: the rarest of its
    // terms by document counts on the first 1/64 of every chunk, then the longest (any choice is correct, a
    // rare one is cheaper).
    let dict = &dict;
    let est = par(nt, |t| {
        let (mut c, mut last, mut p) = (vec![0u32; nterms], vec![NONE; nterms], bs[t]);
        while p < bs[t] + (bs[t + 1] - bs[t]) / 64 {
            let (tab, id) = (find(d, p, b'\t'), p as u32);
            let end = find(d, tab, b'\n');
            tokens(dict, &d[tab + 1..end], |w| {
                if w != NONE && last[w as usize] != id {
                    (last[w as usize] = id, c[w as usize] += 1);
                }
            });
            p = end + 1;
        }
        c
    });
    let mut trig: Vec<Vec<(u32, u32)>> = vec![vec![]; nterms]; // term -> (phrase, offset in the phrase)
    for (k, v) in phr.iter().enumerate() {
        let df = |w: u32| est.iter().map(|c| c[w as usize]).sum::<u32>();
        let j = (0..v.len()).min_by_key(|&j| (df(v[j]), usize::MAX - dict.terms[v[j] as usize].1, j)).unwrap();
        trig[v[j] as usize].push((k as u32, j as u32));
    }
    let tf: Vec<bool> = trig.iter().map(|x| !x.is_empty()).collect(); // 75 KB instead of the list headers
    let (dict, trig, tf, phr, bs, first) = (dict, &trig, &tf, &phr, &bs, &first);
    let parts = par(nt, |t| {
        let range = first[t + 1] - first[t];
        let mut st = vec![[NONE, 0, NONE]; nterms]; // per term: last document, documents, thread bitset
        let (mut stream, mut ends, mut tb) = (vec![], vec![], vec![]);
        let (mut seq, mut hits, mut found, mut lastm) = (vec![], vec![], vec![], vec![NONE; phr.len()]);
        let mut p = bs[t];
        while p < bs[t + 1] {
            let tab = find(d, p, b'\t');
            let id = id_at(p) as u32;
            let end = find(d, tab, b'\n');
            let i = id as usize - first[t];
            tokens(dict, &d[tab + 1..end], |w| {
                let u = w as usize;
                if w != NONE && st[u][0] != id {
                    let e = &mut st[u];
                    e[0] = id;
                    if e[2] == NONE {
                        (e[1] += 1, stream.push(w));
                        if e[1] as usize * 32 >= range {
                            (e[2] = tb.len() as u32, tb.push(vec![0u64; range / 64 + 1]));
                        }
                    } else {
                        tb[e[2] as usize][i / 64] |= 1 << (i % 64);
                    }
                }
                if w != NONE && tf[u] {
                    hits.push((seq.len(), w));
                }
                seq.push(w);
            });
            ends.push(stream.len() as u32);
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
        let (cnt, bit) = (st.iter().map(|e| e[1]).collect(), st.iter().map(|e| e[2]).collect());
        Part { cnt, bit, stream, ends, tb, found }
    });
    let mut pres: Vec<Vec<u32>> = vec![vec![]; phr.len()];
    for x in &parts {
        for &(k, id) in &x.found {
            pres[k as usize].push(id); // threads in order, documents in order: sorted
        }
    }
    // Exact sizes, then each thread lays its stream and bitsets into the final sets: a bitset of all n
    // documents for a term that some thread turned into one or that is long, else a list.
    let words = n / 64 + 1;
    let pr = &parts;
    let meta = par(nt, |c| {
        (c * nterms / nt..(c + 1) * nterms / nt)
            .map(|w| pr.iter().map(|x| x.cnt[w] as usize).sum::<usize>())
            .map(|len| (len, len * 32 >= n))
            .collect::<Vec<_>>()
    })
    .concat();
    let meta: Vec<(usize, bool)> = meta.iter().enumerate().map(|(w, &(l, b))| (l, b || pr.iter().any(|x| x.bit[w] != NONE))).collect();
    // All lists in one zeroed buffer: one mapping, whose pages are touched only as the layout writes them.
    let mut off = vec![0usize; nterms + 1];
    for w in 0..nterms {
        off[w + 1] = off[w] + if meta[w].1 { 0 } else { meta[w].0 };
    }
    let buf: &'static mut [u32] = vec![0u32; off[nterms]].leak();
    let mut bits: Vec<Vec<u64>> = meta.iter().map(|m| if m.1 { vec![0; words] } else { vec![] }).collect();
    let ptrs: Vec<usize> = (0..nterms).map(|w| match meta[w].1 { true => bits[w].as_mut_ptr() as usize, false => buf.as_mut_ptr().wrapping_add(off[w]) as usize }).collect();
    let big: Vec<bool> = meta.iter().map(|m| m.1).collect();
    let (ptrs, big) = (&ptrs, &big);
    par(nt, |t| {
        // where this thread starts in each list: the documents of the threads before it
        let (x, mut cur) = (&pr[t], (0..nterms).map(|w| pr[..t].iter().map(|y| y.cnt[w]).sum()).collect::<Vec<u32>>());
        let set = |w: usize, i: usize| unsafe { (*((ptrs[w] as *const AtomicU64).add(i / 64))).fetch_or(1 << (i % 64), Relaxed) };
        let mut e = 0;
        for (k, &end) in x.ends.iter().enumerate() {
            for &w in &x.stream[e..end as usize] {
                let w = w as usize;
                match big[w] {
                    true => drop(set(w, first[t] + k)),
                    false => unsafe { *(ptrs[w] as *mut u32).add(cur[w] as usize) = (first[t] + k) as u32 },
                }
                cur[w] += !big[w] as u32;
            }
            e = end as usize;
        }
        for w in (0..nterms).filter(|&w| x.bit[w] != NONE) {
            for i in members(&Set::Bits(x.tb[x.bit[w] as usize].clone()), first[t]) {
                set(w, i);
            }
        }
    });
    drop(parts);
    let buf: &'static [u32] = buf;
    let sets: Vec<Set> = bits.into_iter().enumerate().map(|(w, b)| match big[w] { true => Set::Bits(b), false => Set::List(Cow::Borrowed(&buf[off[w]..off[w + 1]])) }).collect();
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
                    r = apply(r, op, &sets[w as usize], words);
                }
                r
            }
            Query::Phrase(k) => Set::List(pres[*k].clone().into()),
        };
        let (count, sum) = match &r {
            Set::List(l) => (l.len() as u64, l.iter().map(|&x| x as u64).sum::<u64>()),
            Set::Bits(b) => b.iter().enumerate().fold((0, 0), |(c, s), (k, &w)| {
                let ones = |j: u32| (w & (!0u64 / ((1u64 << (1 << j)) + 1) << (1 << j))).count_ones() as u64;
                let pos: u64 = (0..6).map(|j| ones(j) << j).sum(); // sum of the set bit positions
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
