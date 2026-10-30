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
   (including every byte of 0x80 and above) separates. Each token is hashed while it is scanned and looked
   up in the dictionary; a query term appends the document id to the thread's posting list for that term
   once per document. Thread lists are concatenated in thread order, so every list is sorted.
3. **BOOL** starts from the first term's list and applies each operator to the running result, strictly
   left to right: merge intersection, merge union, merge difference.
4. **PHRASE, inside the corpus pass.** Each phrase gets one trigger term, its longest term (long words are
   the rare ones; any choice gives the same answer, a rare one is cheaper). While a document is tokenized its
   token ids go to a buffer and the positions of trigger terms are noted; at the end of the document the
   buffer is compared with every phrase of each trigger at the trigger's offset. A repeated term (`w w w`)
   needs no special case: the comparison is on consecutive positions. Matches arrive in document order.
5. **Output.** Queries run on all threads from an atomic counter; answers are stored by query index and
   written in input order.

## Dependencies

None (`dependency_count = 0`).

## With another 24 hours

See `PERF.md`.
