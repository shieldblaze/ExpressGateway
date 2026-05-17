### ROUND8-OPS-12 — Container image lacks RO rootfs + healthcheck; nonroot but writable /tmp; no LABEL provenance

Reference: `audit/round-8/research/pingora.md` (architecture summary — Cloudflare ships Pingora as a library, container hardening is up to embedder); `audit/round-8/research/nginx.md` architecture summary (per-worker isolation as both feature and footgun). Generic OCI / distroless guidance: `gcr.io/distroless/cc-debian12:nonroot` is correct base, but additional flags expected at *image build* time.
Our equivalent: `docker/Dockerfile:30-37`.

Severity: low
Status:   Verified-Fixed(verifier=verify, 66611b91)   <!-- Dockerfile: explicit USER 65532:65532, OCI org.opencontainers.image.* LABEL block; HEALTHCHECK Phase-1-deferred with in-file comment (rec #1 allowed deferral). EXPOSE/RO-rootfs doc are low-sev residuals. Non-blocking. See audit/round-8/verify/ops.md. -->
Status (pre-verify):   Open

Divergence:
- The image correctly uses `gcr.io/distroless/cc-debian12:nonroot` (no shell, no package manager, runs as UID 65532). Good.
- Missing image-level hardening:
  - `HEALTHCHECK` directive — the Dockerfile has no `HEALTHCHECK` instruction. `docker ps` shows "running" but not "healthy". Kubernetes uses its own probes (not Docker HEALTHCHECK), so this is mostly informational; standalone Docker / Compose users lose the signal.
  - No `LABEL org.opencontainers.image.source / version / revision / created` — supply-chain tools (Sigstore, OCI artifacts) rely on these labels for provenance.
  - The config is COPY'd from `/app/config/default.toml` into `/etc/expressgateway/config.toml`. There is no `chmod`/`chown` ensuring the default cert paths referenced in the default config exist or are owned by the nonroot user.
- Missing Dockerfile-level hardening:
  - No `USER` directive in the final stage — distroless `:nonroot` sets it by default but a future base bump could regress; explicit `USER 65532` is the defensive form.
  - No `--mount=type=cache` in the build stage — cargo-chef already partial-fixes the layer-cache story, but `cargo build` step still re-downloads the registry index on cold cache.
  - The image exposes `EXPOSE 80 443 8080 9090` but `lb` does NOT bind 0.0.0.0:80/443 by default; the EXPOSE is for the *example* config. Operators reading the Dockerfile will assume ports 80+443 are bound.
- The image is not configured to use a *read-only* root filesystem at runtime. K8s `securityContext.readOnlyRootFilesystem: true` would require the container to use a tmpfs `/tmp` (or `--read-only --tmpfs /tmp`). The binary itself does not write to `/`, but boring-sys / quiche initialisation might touch `/tmp` — needs verification.

Impact:
- A Docker / Compose user gets a working image but no provenance labels, no healthcheck, and no defaults that match the documented Kubernetes deployment.
- Supply-chain tools (Sigstore-cosign verification, Trivy) emit "no source provenance" warnings.
- The discoverable surface "ports 80/443/8080/9090 EXPOSE" misleads users about the default bind set.

Recommendation:
1. Add `HEALTHCHECK --interval=30s --timeout=3s CMD ["/usr/local/bin/expressgateway", "--healthcheck"]` (requires implementing a `--healthcheck` subcommand that opens 127.0.0.1:`admin_bind`/livez and exits 0 on 200). Defer if subcommand work is non-trivial.
2. Add OCI labels:
   ```dockerfile
   ARG GIT_SHA=unknown
   ARG BUILD_DATE=unknown
   LABEL org.opencontainers.image.source="https://github.com/<org>/ExpressGateway" \
         org.opencontainers.image.revision="${GIT_SHA}" \
         org.opencontainers.image.created="${BUILD_DATE}" \
         org.opencontainers.image.licenses="GPL-3.0-only" \
         org.opencontainers.image.title="expressgateway" \
         org.opencontainers.image.description="L4/L7 LB"
   ```
3. Make the EXPOSE list match the binary's defaults (currently admin on 9090; nothing else binds by default unless config says so). Either drop EXPOSE entirely or document that it's example-config-aspirational.
4. Add `tests/container_smoke.rs` that runs the image with `--read-only --tmpfs /tmp` and asserts `/metrics` answers. This catches "binary writes to /" regressions.
5. Document the K8s `securityContext.readOnlyRootFilesystem: true` requirement in `DEPLOYMENT.md` (currently absent).

Notes:
- Round 1..7 audited the image as "distroless + nonroot" and stopped there. Round-8 adversarial stance: the image is the operator's entry point and the labels / healthcheck / explicit USER are part of the contract.
