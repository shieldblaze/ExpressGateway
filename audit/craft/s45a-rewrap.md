# S45A — comment re-wrap (verifier finding F-3)

The round-1/round-2 comment sweep preserved comment *content* but collapsed multi-line
blocks onto a single physical line. This pass restores the line breaks in the three crates
that were still un-wrapped: `crates/lb`, `crates/lb-l4-xdp`, `crates/lb-soak`.
(`lb-quic` and `lb-l7` were re-wrapped by their round-2 sweepers and were not touched.)

## Before / after

Counted with the verifier's command:

```
for c in lb lb-l4-xdp lb-soak; do
  echo "$c $(grep -rhE '^\s*(//|///|//!)' crates/$c --include='*.rs' | awk 'length>100' | wc -l)"
done
```

| crate | before | after | lines split |
|---|---:|---:|---:|
| `crates/lb` | 96 | **0** | 96 |
| `crates/lb-l4-xdp` | 337 | **0** | 337 |
| `crates/lb-soak` | 101 | **0** | 101 |
| **total** | **534** | **0** | **534** |

51 files changed, 1206 insertions(+), 534 deletions(-) — i.e. every one of the 534
deletions is the over-long line that was replaced by its wrapped form.

(The verifier's table said `lb` had 97; the measurement at the start of this pass was 96.
The one-line difference is commit `dd712889`, which landed between the two measurements.)

## Method

A deterministic script did the wrapping; no line was re-typed by hand, so the comment text
could not drift. Rules encoded in it:

- Only comment-*only* lines (`//`, `///`, `//!`) are candidates. Code is never a candidate,
  so no line of code can be modified.
- The exact indentation + marker of the source line is reproduced on every continuation line.
- Markdown list items (`- `, `* `, `+ `, `N. `, `N) `) indent their continuations under the
  marker, so `clippy::doc_lazy_continuation` does not fire.
- A continuation line is never allowed to *start* with a markdown-significant token
  (`-`, `*`, `+`, `#`, `>`, `|`, `=`, `~`, `N.`, `N)`). If a greedy break would produce one,
  the preceding word is pulled down so the token is no longer first on the line.
- Fenced code blocks inside doc comments (```` ``` ````/`~~~`), markdown table rows, and
  4-space-indented markdown code blocks are skipped.
- Lines that are a single unbreakable token are skipped rather than left mid-word.
- Lines are only ever split; separate comment lines are never merged.

## Proof that only line breaks changed

A second, independent script walked the pre-image (`git archive HEAD`) and the post-image in
lockstep. Every original line must either be reproduced byte-identically, or be an over-long
comment line mapping to N>=2 output lines that carry the *same* prefix and the *identical
word sequence*. Anything else is a violation.

```
=== lb ===         files compared: 14   lines split:  96   violations: 0
=== lb-l4-xdp ===  files compared: 33   lines split: 337   violations: 1   (see below)
=== lb-soak ===    files compared: 13   lines split: 101   violations: 0
```

The check also asserts that no output line exceeds 100 columns and that no continuation line
begins a markdown list. A separate scan confirmed 0 markdown links split between `]` and `(`.

## The one deliberate exception

`crates/lb-l4-xdp/ebpf/src/main.rs:199`

```
// verifier-heavy hot-path read (per-packet `BACKENDS_V4[vip]` + bounded `entries[hash % count]`
// + generation compare) lands with consistent-hash selection; wiring it now would force a
// verifier-log re-capture for a path no production flow exercises yet.
```

The middle line already began with `+ ` in the pre-image — but that `+` is prose (the tail of
an expression continued from the line above), not a markdown bullet. The script's list
detector read it as a bullet and indented the continuation by two spaces. That indent was
removed by hand, which is why the lockstep checker reports one "list continuation not
indented" violation for this line. It is a non-doc `//` comment, so no doc lint applies, and
the pre-existing leading `+` is preserved exactly as the sweep left it.

Nothing else was left over 100 columns: the script reported an empty skip list, so no line
was abandoned as unwrappable.

## Gates

| gate | result |
|---|---|
| `python3 audit/craft/s45a-code-identity.py main` | 5 files differ — **none in `lb`/`lb-l4-xdp`/`lb-soak`** |
| `cargo fmt --all -- --check` | pass (exit 0) |
| `cargo clippy -p lb -p lb-l4-xdp -p lb-soak --all-targets --all-features -- -D warnings` | pass (exit 0) |

Code-identity output:

```
S45A code-identity proof — 254 .rs files changed vs main
  5 file(s) differ: TOKENS DIFFER = real code change; REFLOW ONLY = rustfmt layout, behaviour-neutral
    TOKENS DIFFER  crates/lb-observability/src/xdp_metrics.rs
    TOKENS DIFFER  crates/lb-quic/src/h3_bridge.rs
    TOKENS DIFFER  crates/lb-quic/tests/grpc_h3_e2e.rs
    TOKENS DIFFER  crates/lb-quic/tests/h3_h1_resp_stream_e2e.rs
    TOKENS DIFFER  crates/lb-quic/tests/h3_h3_stream_e2e.rs
```

`crates/lb/src/main.rs` had been on this list as a rustfmt enum reflow; commit `dd712889`
(verifier F-1/F-2 restores) cleared it before this pass ran. It is **not** on the list after
the re-wrap, so this pass introduced no code difference in it — nor in any file it touched.

## Note on `crates/lb-l4-xdp/ebpf`

The eBPF crate is a standalone crate, deliberately outside the root workspace members list
(it targets `bpfel-unknown-none` and needs nightly + bpf-linker). Its 55 wrapped lines are
therefore covered by the lockstep text-identity proof but **not** by the `cargo fmt` /
`cargo clippy` gates above. All of its changes are `//` line comments, where no doc lint
applies.
