# Track B: Index

The task, the input files and the exact output format are in the [spec](https://github.com/hackathon-raptors/speed-demon-2026/blob/main/specs/b-index.md).

The code was written with the help of Claude Code (Anthropic), as Rule 06 allows. Every change is
measured before it is kept; `PERF.md` lists the steps.

## Approach

Rust, standard library only, one file (`src/main.rs`).

1. **Queries first.** `queries.txt` is parsed before the corpus. Every distinct term gets an id in an
   open-addressing dictionary; only these terms are indexed (section 3 allows it).
2. **Corpus, all threads.** `docs.txt` is mapped and cut at line starts, one chunk per thread. The tokenizer
   is the spec's byte rule, eight bytes at a time (SWAR): ASCII-lowercase, a token is a maximal run of `a-z0-9`, every other byte
   (including every byte of 0x80 and above) separates. The tokenizer runs one token ahead: the next token is found and
   hashed and its dictionary slot prefetched before the current one is looked up. Each document's TAB and LF
   are found eight bytes at a time (SWAR). Per document, the thread appends the ids of its distinct query terms to one flat
   stream and counts documents per term; a term that passes 1/32 of the thread's documents goes to a bitset
   of the thread's range instead. From the counts, every term gets an exact list (or, if frequent, a bitset
   of all documents), and each thread lays its stream into it at its own offset, so lists come out sorted.
3. **BOOL** starts from the first term's set and applies each operator to the running result, strictly
   left to right: galloping intersection and difference and merge union on lists; AND of a list with a
   bitset keeps the list side (filtered by bit tests), OR and NOT on a bitset work in place, and only
   list OR bitset copies a bitset.
4. **PHRASE, inside the corpus pass.** Each phrase gets one trigger term: the rarest of its terms by document
   counts on the first 1/64 of every chunk (a parallel pre-pass of about 4 MB), ties broken by length (any
   choice gives the same answer, a rare one is cheaper: the first version took the longest term, and checking
   phrases at common words was a quarter of the corpus pass). While a document is tokenized its
   token ids go to a buffer and the positions of trigger terms are noted; at the end of the document the
   buffer is compared with every phrase of each trigger at the trigger's offset. A repeated term (`w w w`)
   needs no special case: the comparison is on consecutive positions. Matches arrive in document order.
5. **Output.** Queries run on all threads from an atomic counter; answers are stored by query index and
   written in input order.

## Memory

The thread streams hold `u16` term ranks (terms ranked by the sampled counts, an escape for the rare tail).
All final lists live in one buffer, touched only by the layout; a term's list borrows its slice of the
buffer, so a query that starts from a term copies nothing. Frequent terms are bitsets of all documents (31 KB each).

## Dependencies

None (`dependency_count = 0`).

## With another 24 hours

See `PERF.md`.
