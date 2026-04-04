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
package com.shieldblaze.expressgateway.protocol.quic.nat;

import com.shieldblaze.expressgateway.protocol.quic.QuicBackendSession;
import org.apache.logging.log4j.LogManager;
import org.apache.logging.log4j.Logger;

import java.net.InetAddress;
import java.net.InetSocketAddress;
import java.util.Map;
import java.util.concurrent.ConcurrentHashMap;
import java.util.concurrent.atomic.AtomicLong;

/**
 * Detects and handles NAT rebinding events per RFC 9000 Section 9.3.
 *
 * <h3>NAT Rebinding vs Connection Migration</h3>
 * <p>NAT rebinding occurs when a NAT device assigns a new source port to an existing
 * mapping. The client's IP address stays the same, but the port changes. This is
 * distinct from full connection migration where the IP address changes (e.g., WiFi
 * to cellular).</p>
 *
 * <p>RFC 9000 Section 9.3.1: "An endpoint MAY skip validation for a peer address
 * change that is only a change in port. Such a change is likely to be caused by
 * a NAT rebinding or other middlebox activity."</p>
 *
 * <h3>Detection Algorithm</h3>
 * <p>NAT rebinding is classified when:
 * <ul>
 *   <li>The IP address is identical</li>
 *   <li>Only the port has changed</li>
 *   <li>The rebinding is not too frequent (anti-spoofing protection)</li>
 * </ul>
 * When detected, the address mapping is updated without requiring PATH_CHALLENGE
 * validation, providing lower latency than full migration.</p>
 *
 * <h3>Anti-Spoofing</h3>
 * <p>To prevent spoofed NAT rebinding from hijacking connections:
 * <ul>
 *   <li>Rate limits rebinding events per IP address</li>
 *   <li>Tracks rebinding frequency to detect anomalous patterns</li>
 *   <li>Optionally requires lightweight validation for high-frequency rebindings</li>
 * </ul></p>
 *
 * <h3>Thread Safety</h3>
 * <p>All operations are lock-free using {@link ConcurrentHashMap}.</p>
 */
public final class NatRebindingDetector {

    private static final Logger logger = LogManager.getLogger(NatRebindingDetector.class);

    /**
     * Maximum rebindings per IP address within the rate window before requiring
     * full path validation. Default: 5 rebindings per window.
     */
    private final int maxRebindingsPerWindow;

    /**
     * Rate window in nanoseconds. Default: 10 seconds.
     */
    private final long rateWindowNanos;

    /**
     * Tracks rebinding frequency per IP address for anti-spoofing.
     * Key: IP address, Value: rebinding counter state.
     */
    private final ConcurrentHashMap<InetAddress, RebindingCounter> rebindingCounters = new ConcurrentHashMap<>();

    private final AtomicLong totalRebindings = new AtomicLong();
    private final AtomicLong rejectedRebindings = new AtomicLong();

    /**
     * Rebinding rate counter per IP address.
     *
     * @param count number of rebindings in the current window
     * @param windowStartNanos start of the current rate window
     */
    private record RebindingCounter(int count, long windowStartNanos) {

        RebindingCounter increment(long now, long windowNanos) {
            if (now - windowStartNanos > windowNanos) {
                // Window expired, start new window
                return new RebindingCounter(1, now);
            }
            return new RebindingCounter(count + 1, windowStartNanos);
        }

        boolean isOverLimit(int maxPerWindow) {
            return count > maxPerWindow;
        }
    }

    /**
     * Create a NAT rebinding detector with default settings.
     * Max 5 rebindings per 10 second window.
     */
    public NatRebindingDetector() {
        this(5, 10_000_000_000L);
    }

    /**
     * Create a NAT rebinding detector with custom rate limiting.
     *
     * @param maxRebindingsPerWindow max rebindings allowed per rate window
     * @param rateWindowNanos        rate window duration in nanoseconds
     */
    public NatRebindingDetector(int maxRebindingsPerWindow, long rateWindowNanos) {
        if (maxRebindingsPerWindow <= 0) {
            throw new IllegalArgumentException("maxRebindingsPerWindow must be positive");
        }
        if (rateWindowNanos <= 0) {
            throw new IllegalArgumentException("rateWindowNanos must be positive");
        }
        this.maxRebindingsPerWindow = maxRebindingsPerWindow;
        this.rateWindowNanos = rateWindowNanos;
    }

    /**
     * Determine if an address change is NAT rebinding (port-only change on same IP).
     *
     * @param oldAddress the previous client address
     * @param newAddress the new client address
     * @return true if the change is a port-only change (NAT rebinding), false if IP changed
     */
    public boolean isNatRebinding(InetSocketAddress oldAddress, InetSocketAddress newAddress) {
        if (oldAddress == null || newAddress == null) {
            return false;
        }
        // Same IP, different port = NAT rebinding per RFC 9000 Section 9.3.1
        return oldAddress.getAddress().equals(newAddress.getAddress())
                && oldAddress.getPort() != newAddress.getPort();
    }

    /**
     * Handle a detected NAT rebinding event.
     *
     * <p>Updates the address-to-session mapping for the new port without requiring
     * PATH_CHALLENGE validation, per RFC 9000 Section 9.3.1 which allows skipping
     * validation for port-only changes.</p>
     *
     * <p>The old address is removed from the address-to-session map before adding
     * the new one, preventing stale address mappings from accumulating.</p>
     *
     * <p>Rate limiting is enforced atomically inside compute() to avoid TOCTOU races
     * where concurrent rebindings could both pass the check before either increments.</p>
     *
     * @param oldAddress      the previous client address
     * @param newAddress      the new client address (same IP, different port)
     * @param session         the backend session for this connection
     * @param addressSessionMap the address-to-session map to update
     * @return true if the rebinding was accepted, false if rate-limited
     */
    public boolean handleRebinding(InetSocketAddress oldAddress, InetSocketAddress newAddress,
                                    QuicBackendSession session,
                                    Map<InetSocketAddress, QuicBackendSession> addressSessionMap) {
        InetAddress clientIp = newAddress.getAddress();
        long now = System.nanoTime();

        // Atomically check rate limit AND increment counter inside compute() to prevent TOCTOU
        boolean[] allowed = {false};
        rebindingCounters.compute(clientIp, (ip, existing) -> {
            RebindingCounter updated;
            if (existing == null) {
                updated = new RebindingCounter(1, now);
            } else {
                updated = existing.increment(now, rateWindowNanos);
            }
            if (updated.isOverLimit(maxRebindingsPerWindow)) {
                // Over limit: return the updated counter (with incremented count) but flag as rejected
                allowed[0] = false;
                return updated;
            }
            allowed[0] = true;
            return updated;
        });

        if (!allowed[0]) {
            rejectedRebindings.incrementAndGet();
            logger.warn("NAT rebinding rate-limited for IP {}", clientIp);
            return false;
        }

        // Put new mapping first to avoid a window where neither address resolves.
        // Then remove old mapping to prevent stale entries from accumulating.
        addressSessionMap.put(newAddress, session);
        addressSessionMap.remove(oldAddress);

        totalRebindings.incrementAndGet();
        if (logger.isDebugEnabled()) {
            logger.debug("NAT rebinding handled: {} -> {} (port change only)", oldAddress, newAddress);
        }

        return true;
    }

    /**
     * Check if an IP address has exceeded the rebinding rate limit.
     *
     * @param address the address to check
     * @return true if the address is currently rate-limited
     */
    public boolean isRateLimited(InetSocketAddress address) {
        RebindingCounter counter = rebindingCounters.get(address.getAddress());
        if (counter == null) {
            return false;
        }
        // Check if window expired
        if (System.nanoTime() - counter.windowStartNanos() > rateWindowNanos) {
            return false;
        }
        // Check if the count has reached the limit (next attempt would be rejected)
        return counter.count() >= maxRebindingsPerWindow;
    }

    /**
     * Evict expired rate limiting entries.
     *
     * @return number of expired entries removed
     */
    public int evictExpired() {
        long now = System.nanoTime();
        int[] evicted = {0};
        rebindingCounters.entrySet().removeIf(e -> {
            if (now - e.getValue().windowStartNanos() > rateWindowNanos * 6) {
                evicted[0]++;
                return true;
            }
            return false;
        });
        return evicted[0];
    }

    public long totalRebindings() {
        return totalRebindings.get();
    }

    public long rejectedRebindings() {
        return rejectedRebindings.get();
    }
}
