# Plan for EBPF-2-02 — `#[link_section = "license"]` static (MERGED INTO EBPF-2-01)
Finding-ref:     EBPF-2-02 (medium, Open)
Files touched:   (see `audit/ebpf/plans/EBPF-2-01.md`)
Approach:        **Merged into EBPF-2-01 per lead instruction.** Both
                 findings share the same source file
                 (`crates/lb-l4-xdp/ebpf/src/main.rs`) and the same
                 mechanism (the `#[unsafe(link_section = "license")]`
                 static `LICENSE: [u8; 4] = *b"GPL\0"`). The license
                 section IS the mechanism EBPF-2-01 demands; the BTF
                 emission piece is layered on top of the same build
                 reorganisation. Splitting the plan would force two
                 reviewers to touch the same file in Round 4, which
                 the lead-mandated ownership matrix forbids.

                 EBPF-2-02 closes the moment EBPF-2-01 lands, because
                 the rebuild that adds the BTF sections also emits
                 the `license` section sourced from the new static.

                 The round-2 review file already records the explicit
                 finding that aya 0.13.1 has no `set_license`
                 setter (this is the disposition correction the
                 audit-brief asked for). No source-side action is
                 required beyond what EBPF-2-01 lands.

Proof:           Same as EBPF-2-01:
                 `lb-l4-xdp/tests/elf_sections.rs::elf_has_license_btf_and_size_budget`
                 asserts `readelf -p license` returns `"GPL"`. That
                 single assertion satisfies both findings.

Risk / blast radius: None additional beyond EBPF-2-01's blast radius.

Cross-ref:       EBPF-2-01 (merged-into), SEC-2-12 (downgraded by
                 lead synthesis §A; closed by the explicit section
                 this plan ships).

Owner:           ebpf
Lead-approval: approved 2026-05-13 team-lead
