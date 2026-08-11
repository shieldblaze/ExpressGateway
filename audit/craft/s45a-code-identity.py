#!/usr/bin/env python3
"""S45A behavior-neutrality proof (R3/R13).

A comment pass must not change code. Reviewing a 20k-line diff by eye cannot
establish that. This strips every comment from both the baseline and the current
tree and compares what is left, token-for-token modulo whitespace.

Any file whose stripped source differs is a CODE change and must be justified
individually. This is what caught the deleted `#[map(name = ...)]` attribute.

Usage:  audit/craft/s45a-code-identity.py [baseline-ref]      (default: main)
"""
import subprocess
import sys

BASE = sys.argv[1] if len(sys.argv) > 1 else "main"


def strip_comments(src: str) -> str:
    """Remove Rust comments. Handles strings, chars, raw strings, nesting."""
    out = []
    i, n = 0, len(src)
    while i < n:
        c = src[i]
        # raw string: r"..." / r#"..."#
        if c == "r" and i + 1 < n and src[i + 1] in '"#':
            j = i + 1
            hashes = 0
            while j < n and src[j] == "#":
                hashes += 1
                j += 1
            if j < n and src[j] == '"':
                term = '"' + "#" * hashes
                end = src.find(term, j + 1)
                end = n if end == -1 else end + len(term)
                out.append(src[i:end])
                i = end
                continue
        if c == '"':  # string literal
            j = i + 1
            while j < n:
                if src[j] == "\\":
                    j += 2
                    continue
                if src[j] == '"':
                    j += 1
                    break
                j += 1
            out.append(src[i:j])
            i = j
            continue
        if c == "'":  # char literal or lifetime
            j = i + 1
            while j < n:
                if src[j] == "\\":
                    j += 2
                    continue
                if src[j] == "'":
                    j += 1
                    break
                if src[j] in " \t\r\n;,>)":  # lifetime, not a char literal
                    break
                j += 1
            out.append(src[i:j])
            i = j
            continue
        if c == "/" and i + 1 < n and src[i + 1] == "/":
            j = src.find("\n", i)
            i = n if j == -1 else j
            continue
        if c == "/" and i + 1 < n and src[i + 1] == "*":
            depth, j = 1, i + 2
            while j < n and depth:
                if src.startswith("/*", j):
                    depth += 1
                    j += 2
                elif src.startswith("*/", j):
                    depth -= 1
                    j += 2
                else:
                    j += 1
            i = j
            continue
        out.append(c)
        i += 1
    # collapse whitespace so reflowed code compares equal only if truly identical
    return "\n".join(" ".join(ln.split()) for ln in "".join(out).split("\n") if ln.strip())


def git(*args) -> str:
    return subprocess.run(["git", *args], capture_output=True, text=True).stdout


files = [f for f in git("diff", "--name-only", BASE, "--", "*.rs").split("\n") if f.strip()]
if not files:
    print("no .rs files changed vs", BASE)
    sys.exit(0)

changed = []
for f in files:
    old = git("show", f"{BASE}:{f}")
    try:
        new = open(f, errors="ignore").read()
    except FileNotFoundError:
        changed.append((f, "DELETED"))
        continue
    if strip_comments(old) != strip_comments(new):
        changed.append((f, "CODE DIFFERS"))

print(f"S45A code-identity proof — {len(files)} .rs files changed vs {BASE}")
if not changed:
    print(f"  PASS: all {len(files)} files are COMMENT-ONLY changes "
          "(stripped source is identical).")
    sys.exit(0)

print(f"  {len(changed)} file(s) with real code changes — each needs justification:")
for f, why in changed:
    print(f"    {why:14s} {f}")
sys.exit(1)
