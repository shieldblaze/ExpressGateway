/*
 * This file is part of ShieldBlaze ExpressGateway. [www.shieldblaze.com]
 * Copyright (c) 2020-2022 ShieldBlaze
 *
 * ShieldBlaze ExpressGateway is free software: you can redistribute it and/or modify
 * it under the terms of the GNU General Public License as published by
 * the Free Software Foundation, either version 3 of the License, or
 * (at your option) any later version.
 *
 * ShieldBlaze ExpressGateway is distributed in the hope that it will be useful,
 * but WITHOUT ANY WARRANTY; without even the implied warranty of
 * MERCHANTABILITY or FITNESS FOR A PARTICULAR PURPOSE.  See the
 * GNU General Public License for more details.
 *
 * You should have received a copy of the GNU General Public License
 * along with ShieldBlaze ExpressGateway.  If not, see <https://www.gnu.org/licenses/>.
 */
package com.shieldblaze.expressgateway.security;

import com.google.common.cache.Cache;
import com.google.common.cache.CacheBuilder;
import io.github.bucket4j.Bandwidth;
import io.github.bucket4j.Bucket;
import io.netty.channel.ChannelHandler;
import io.netty.channel.ChannelHandlerContext;
import io.netty.handler.ipfilter.IpSubnetFilter;
import io.netty.handler.ipfilter.IpSubnetFilterRule;
import org.apache.logging.log4j.LogManager;
import org.apache.logging.log4j.Logger;

import java.net.Inet6Address;
import java.net.InetAddress;
import java.net.InetSocketAddress;
import java.time.Duration;
import java.util.Collections;
import java.util.List;
import java.util.Objects;
import java.util.concurrent.ExecutionException;
import java.util.concurrent.atomic.LongAdder;

/**
 * Network Access Control List with radix-trie based IP matching,
 * allowlist/denylist modes, per-rule hit counters, and per-IP rate limiting.
 *
 * <p>The ACL supports two modes:</p>
 * <ul>
 *   <li>{@link Mode#ALLOWLIST}: Only IPs matching an Allow rule are accepted. Everything else is denied.</li>
 *   <li>{@link Mode#DENYLIST}: IPs matching a Deny rule are rejected. Everything else is accepted.</li>
 * </ul>
 *
 * <p>IP matching uses a binary radix trie for O(prefix_length) lookups -- at most
 * 32 steps for IPv4 and 128 for IPv6 -- instead of O(n) linear scan over rules.</p>
 *
 * <p>Dynamic rule updates are achieved via copy-on-write: {@link #updateRules(List)}
 * builds a new trie and swaps it atomically. Read operations on the hot path never
 * block or contend.</p>
 *
 * <p>Backward compatibility: The original constructors accepting {@link IpSubnetFilterRule}
 * lists still work and delegate to Netty's {@link IpSubnetFilter} for legacy rule evaluation.
 * The new radix trie rules are evaluated in addition to (not replacing) the legacy path.</p>
 */
@ChannelHandler.Sharable
public final class NACL extends IpSubnetFilter {

    private static final Logger logger = LogManager.getLogger(NACL.class);

    /**
     * ACL operating mode.
     */
    public enum Mode {
        /** Only matching Allow rules permit traffic. Default deny. */
        ALLOWLIST,
        /** Matching Deny rules block traffic. Default allow. */
        DENYLIST
    }

    private final Bucket globalBucket;
    private final int perIpConnections;
    private final Duration perIpDuration;

    static final int DEFAULT_MAX_PER_IP_ENTRIES = 100_000;

    private final Cache<String, Bucket> perIpBuckets;

    /**
     * Immutable holder for the trie state. A single volatile reference ensures
     * that accept() always sees a consistent snapshot of (ipv4Trie, ipv6Trie, mode)
     * without the race window that three separate volatile fields would create.
     */
    private record TrieState(IpRadixTrie ipv4Trie, IpRadixTrie ipv6Trie, Mode mode) {}

    /**
     * Radix trie state for O(prefix_length) CIDR matching. Accessed via volatile read
     * on the hot path; updated via copy-on-write in {@link #updateRules(List)}.
     * The record groups ipv4Trie, ipv6Trie, and mode into a single atomic reference
     * to prevent torn reads when updateRules() and setMode() are called concurrently.
     */
    private volatile TrieState trieState;

    private final LongAdder totalAccepted = new LongAdder();
    private final LongAdder totalDenied = new LongAdder();

    // ========================================================================
    // Backward-compatible constructors (legacy IpSubnetFilterRule path)
    // ========================================================================

    /**
     * Create a new {@link NACL} Instance with Rate-Limit Disabled.
     *
     * @param ipSubnetFilterRules {@link List} of {@link IpSubnetFilterRule}
     * @param acceptIfNotFound    Set to {@code true} to accept connections not matching any rule
     */
    public NACL(List<IpSubnetFilterRule> ipSubnetFilterRules, boolean acceptIfNotFound) {
        this(0, null, ipSubnetFilterRules, acceptIfNotFound);
    }

    /**
     * Create a new {@link NACL} Instance with ACL Disabled.
     *
     * @param connections Number of connections for Rate-Limit
     * @param duration    {@link Duration} of Rate-Limit
     */
    public NACL(int connections, Duration duration) {
        this(connections, duration, Collections.emptyList(), true);
    }

    /**
     * Create a new {@link NACL} Instance.
     *
     * @param connections         Number of connections for Rate-Limit
     * @param duration            {@link Duration} of Rate-Limit
     * @param ipSubnetFilterRules {@link List} of {@link IpSubnetFilterRule}
     * @param acceptIfNotFound    Set to {@code true} to accept connections not matching any rule
     */
    public NACL(int connections, Duration duration, List<IpSubnetFilterRule> ipSubnetFilterRules,
                boolean acceptIfNotFound) {
        this(connections, duration, 0, null, ipSubnetFilterRules, acceptIfNotFound,
                Collections.emptyList(), acceptIfNotFound ? Mode.DENYLIST : Mode.ALLOWLIST,
                DEFAULT_MAX_PER_IP_ENTRIES);
    }

    // ========================================================================
    // New constructors with radix-trie rules and mode
    // ========================================================================

    /**
     * Full constructor with all parameters.
     *
     * @param globalConnections   Global rate limit connections (0 to disable)
     * @param globalDuration      Global rate limit duration (null to disable)
     * @param perIpConnections    Per-IP rate limit connections (0 to disable)
     * @param perIpDuration       Per-IP rate limit duration (null to disable)
     * @param legacyRules         Legacy Netty IpSubnetFilterRule list
     * @param acceptIfNotFound    Legacy accept-if-not-found behavior
     * @param naclRules           New NACL rules for radix trie
     * @param mode                ACL operating mode
     * @param maxPerIpEntries     Maximum tracked per-IP buckets
     */
    public NACL(int globalConnections, Duration globalDuration,
                int perIpConnections, Duration perIpDuration,
                List<IpSubnetFilterRule> legacyRules, boolean acceptIfNotFound,
                List<NACLRule> naclRules, Mode mode, int maxPerIpEntries) {
        super(acceptIfNotFound, legacyRules);

        // Build radix tries from initial rules
        IpRadixTrie v4 = new IpRadixTrie();
        IpRadixTrie v6 = new IpRadixTrie();
        for (NACLRule rule : naclRules) {
            if (rule.network().length == 4) {
                v4.insert(rule);
            } else {
                v6.insert(rule);
            }
        }
        this.trieState = new TrieState(v4, v6, mode);

        // Global rate limit
        if (globalConnections > 0 && globalDuration != null) {
            if (globalDuration.isZero() || globalDuration.isNegative()) {
                throw new IllegalArgumentException("globalDuration must be positive, was: " + globalDuration);
            }
            Bandwidth limit = Bandwidth.simple(globalConnections, globalDuration);
            globalBucket = Bucket.builder().addLimit(limit).withNanosecondPrecision().build();
            logger.info("Global connection Rate-Limit: {} connection(s) in {}", globalConnections, globalDuration);
        } else {
            globalBucket = null;
            logger.info("Global connection Rate-Limit is Disabled");
        }

        // Per-IP rate limit
        if (perIpConnections > 0 && perIpDuration != null) {
            this.perIpConnections = perIpConnections;
            this.perIpDuration = perIpDuration;
        } else if (globalConnections > 0 && globalDuration != null) {
            // Backward compat: use global params for per-IP when not explicitly configured
            this.perIpConnections = globalConnections;
            this.perIpDuration = globalDuration;
        } else {
            this.perIpConnections = 0;
            this.perIpDuration = null;
        }

        this.perIpBuckets = CacheBuilder.newBuilder()
                .maximumSize(maxPerIpEntries)
                .expireAfterAccess(Duration.ofHours(1))
                .build();

        if (this.perIpConnections > 0) {
            logger.info("Per-IP connection Rate-Limit: {} connection(s) in {} (max {} tracked IPs)",
                    this.perIpConnections, this.perIpDuration, maxPerIpEntries);
        }
    }

    @Override
    protected boolean accept(ChannelHandlerContext ctx, InetSocketAddress remoteAddress) {
        try {
            // 1. Global rate limit check (cheapest check first)
            if (globalBucket != null && !globalBucket.tryConsume(1)) {
                logger.debug("Global Rate-Limit exceeded, denying connection from {}", remoteAddress);
                totalDenied.increment();
                return false;
            }

            // 2. Per-IP rate limit check
            if (perIpConnections > 0 && perIpDuration != null) {
                String key = rateLimitKey(remoteAddress.getAddress());
                Bucket ipBucket;
                try {
                    ipBucket = perIpBuckets.get(key, () -> {
                        Bandwidth limit = Bandwidth.simple(perIpConnections, perIpDuration);
                        return Bucket.builder().addLimit(limit).withNanosecondPrecision().build();
                    });
                } catch (ExecutionException e) {
                    logger.warn("Failed to create per-IP bucket for {}", remoteAddress, e);
                    totalDenied.increment();
                    return false;
                }
                if (!ipBucket.tryConsume(1)) {
                    logger.debug("Per-IP Rate-Limit exceeded for {}, denying connection", remoteAddress);
                    totalDenied.increment();
                    return false;
                }
            }

            // 3. Radix trie ACL check (O(prefix_length) lookup)
            // Read the trie state once into a local variable for a consistent snapshot
            TrieState snapshot = this.trieState;
            byte[] addrBytes = remoteAddress.getAddress().getAddress();
            IpRadixTrie trie = (addrBytes.length == 4) ? snapshot.ipv4Trie() : snapshot.ipv6Trie();
            NACLRule matchedRule = trie.longestPrefixMatch(addrBytes);

            if (matchedRule != null) {
                matchedRule.hitCounter().increment();
                boolean accepted = switch (snapshot.mode()) {
                    case ALLOWLIST -> matchedRule instanceof NACLRule.Allow;
                    case DENYLIST -> !(matchedRule instanceof NACLRule.Deny);
                };
                if (!accepted) {
                    logger.debug("ACL rule denied connection from {}", remoteAddress);
                    totalDenied.increment();
                    return false;
                }
                totalAccepted.increment();
                // Trie rule matched and allowed; still run legacy Netty filter
                return super.accept(ctx, remoteAddress);
            }

            // No trie rule matched: apply mode default
            boolean defaultAccept = switch (snapshot.mode()) {
                case ALLOWLIST -> false; // default deny in allowlist mode
                case DENYLIST -> true;   // default allow in denylist mode
            };

            if (!defaultAccept) {
                logger.debug("No ACL rule matched for {} in {} mode, denying", remoteAddress, snapshot.mode());
                totalDenied.increment();
                return false;
            }

            // 4. Fall through to legacy Netty IpSubnetFilter
            boolean legacyResult = super.accept(ctx, remoteAddress);
            if (legacyResult) {
                totalAccepted.increment();
            } else {
                totalDenied.increment();
            }
            return legacyResult;
        } catch (Exception ex) {
            logger.warn("Error evaluating ACL for {}", remoteAddress, ex);
            totalDenied.increment();
        }
        return false;
    }

    /**
     * Atomically replace all NACL rules. Builds a new radix trie and swaps it in.
     * No lock contention on the read path -- readers see either the old or new trie.
     *
     * @param newRules the new rule set
     */
    public void updateRules(List<NACLRule> newRules) {
        IpRadixTrie v4 = new IpRadixTrie();
        IpRadixTrie v6 = new IpRadixTrie();
        for (NACLRule rule : newRules) {
            if (rule.network().length == 4) {
                v4.insert(rule);
            } else {
                v6.insert(rule);
            }
        }
        // Atomically swap all three fields via the single TrieState reference
        this.trieState = new TrieState(v4, v6, this.trieState.mode());
        logger.info("ACL rules updated: {} rules loaded", newRules.size());
    }

    /**
     * Change the operating mode dynamically.
     */
    public void setMode(Mode mode) {
        Objects.requireNonNull(mode, "mode");
        // Atomically swap the mode while preserving the current tries
        TrieState current = this.trieState;
        this.trieState = new TrieState(current.ipv4Trie(), current.ipv6Trie(), mode);
        logger.info("ACL mode changed to {}", mode);
    }

    /**
     * Returns the current operating mode.
     */
    public Mode getMode() {
        return trieState.mode();
    }

    /**
     * Returns the number of tracked per-IP rate limit buckets.
     */
    public long trackedIpCount() {
        return perIpBuckets.size();
    }

    /**
     * Returns the total number of accepted connections.
     */
    public long totalAccepted() {
        return totalAccepted.sum();
    }

    /**
     * Returns the total number of denied connections.
     */
    public long totalDenied() {
        return totalDenied.sum();
    }

    /**
     * Returns a snapshot of all current IPv4 rules.
     */
    public List<NACLRule> ipv4Rules() {
        return trieState.ipv4Trie().allRules();
    }

    /**
     * Returns a snapshot of all current IPv6 rules.
     */
    public List<NACLRule> ipv6Rules() {
        return trieState.ipv6Trie().allRules();
    }

    /**
     * Compute the rate limit key for a given address.
     * For IPv6 addresses, masks to /64 to prevent trivial bypass via address rotation.
     * For IPv4 addresses, uses the full address.
     */
    static String rateLimitKey(InetAddress address) {
        if (address instanceof Inet6Address) {
            byte[] raw = address.getAddress();
            StringBuilder sb = new StringBuilder(23);
            for (int i = 0; i < 8; i++) {
                if (i > 0) sb.append(':');
                sb.append(String.format("%02x", raw[i]));
            }
            return sb.append("::/64").toString();
        }
        return address.getHostAddress();
    }
}
