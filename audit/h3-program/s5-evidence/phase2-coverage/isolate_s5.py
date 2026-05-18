#!/usr/bin/env python3
# Phase 2 — isolate S5-product line coverage from s5.lcov.
# S5 ranges derived from: git diff bd2e6dca..2fa417aa -- \
#   crates/lb-quic/src/conn_actor.rs crates/lb-quic/src/h3_bridge.rs
import sys
lcov=sys.argv[1] if len(sys.argv)>1 else "s5.lcov"
cov={};cur=None
for ln in open(lcov):
    ln=ln.rstrip("\n")
    if ln.startswith("SF:"): cur=ln[3:]; cov.setdefault(cur,{})
    elif ln.startswith("DA:") and cur:
        a,b=ln[3:].split(",")[:2]; cov[cur][int(a)]=int(b)
ca=[k for k in cov if k.endswith("conn_actor.rs")][0]
hb=[k for k in cov if k.endswith("h3_bridge.rs")][0]
s5={"conn_actor.rs":{"reap call site (282-286)":(ca,range(282,287)),
 "reap_client_cancelled_responses fn (317-344)":(ca,range(317,345))},
 "h3_bridge.rs":{"encode_h3_trailers_frame (912-917)":(hb,range(912,918)),
 "ChunkDecoder::new C4 inits (988-990)":(hb,range(988,991)),
 "take_trailers (997-999)":(hb,range(997,1000)),
 "feed C4 dispatch + parse_trailer_section (1008-1149)":(hb,range(1008,1150)),
 "chunked-arm while !complete + trailer emit (1382-1428)":(hb,range(1382,1429))}}
te=th=0
for f,g in s5.items():
    print("=== %s ==="%f)
    for lbl,(fp,rng) in g.items():
        ex=[l for l in rng if l in cov[fp]]
        h=[l for l in ex if cov[fp][l]>0]; m=[l for l in ex if cov[fp][l]==0]
        te+=len(ex); th+=len(h)
        print("  %s: %d/%d = %.1f%%%s"%(lbl,len(h),len(ex),
              100.0*len(h)/len(ex) if ex else 100.0,
              "  MISSING=%s"%m if m else ""))
print("\nISOLATED S5 AGGREGATE: %d/%d = %.2f%%"%(th,te,100.0*th/te))
