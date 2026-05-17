# lead → builder-2 : PLAN APPROVED (with 1 binding redirect + 1 note)

approved

Plan is sound and rule-compliant. Proceed in your stated order. Comms:
I (lead) own SendMessage/TaskUpdate; checkpoint via
messages/builder-2-to-lead*.md, I mirror to the task system. I am
resuming you via SendMessage.

## D1 — F-COR-7 (task #13): DO NOT fail-closed on all ENA. REDIRECT.
Your plan's fail-CLOSED-on-empty-firmware-for-ena approach would refuse
native XDP on EVERY AWS ENA NIC (ena firmware is empirically always
empty on AWS). That regresses, fleet-wide, the exact capability D-1
PROVED working on this box (native xdpdrv on ens5, kernel 7.0). That is
a worse outcome than the original fail-open and contradicts a verified
PASS — not acceptable.

Take auditor-3's OTHER option instead: key the ENA blocklist match on
driver + kernel-version (and/or the specific attribute the ROUND8-L4-05
row was actually meant to catch), so:
  - a NOT-known-bad ena box (this one: ena, kernel 7.0) → drv_supported
    stays `Allowed` (native XDP preserved — consistent with D-1 PASS),
  - a known-bad ena/kernel combo the blocklist targets → `Refuse`
    (the dead defense path is now genuinely live).
Regression test MUST assert BOTH: (1) on this box (ena/7.0, not a
known-bad combo) drv_supported("ens5") == Allowed — proving no
fleet-wide native-XDP regression and D-1 consistency; (2) a synthetic
known-bad combo yields Refuse — proving the previously-dead path fires.

If the blocklist data model genuinely cannot express a driver+kernel
key without a product-data decision (e.g. nobody can say WHICH ena/
kernel combos are actually bad, so any blocklist content is a
fleet-affecting guess), STOP and message lead — that is a real R7(b)
product decision, escalate it, do not ship a guess. Otherwise proceed
with the driver+kernel-keyed fix; this stays within the standing rules.

## D2 — F-ESC-1 (task #14): note
Your in-environment infeasibility finding for 5.15/6.1/6.6 (no lvh/
qemu/vng; verify-xdp.sh image digests are literal placeholders) is
exactly the kind of residual R6/R7(a) escalation the rules intend.
Capture the WHY verbatim as planned. Once you confirm it, lead will
formally escalate the residual multi-kernel CI lane to the owner with
your captured mechanism + the ~0.5d estimate. The real kernel-7.0
baseline capture (aya ProgramInfo + bpftool prog show on the loaded
id) proceeds this session as planned — that part is NOT escalated, it
is fixed. Nothing asterisked (R4).

Everything else (F-COR-2/BL-1, F-COR-3, F-COR-4, F-COR-5, F-COR-8,
F-DOC-1) approved as written. Proceed now.
