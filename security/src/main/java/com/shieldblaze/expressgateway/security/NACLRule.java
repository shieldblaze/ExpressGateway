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

import java.net.InetAddress;
import java.net.UnknownHostException;
import java.util.concurrent.atomic.LongAdder;

/**
 * Sealed interface representing a Network Access Control List rule.
 * Rules are either {@link Allow} or {@link Deny}, each carrying a
 * CIDR network prefix and an atomic hit counter.
 */
public sealed interface NACLRule permits NACLRule.Allow, NACLRule.Deny {

    /**
     * The network address (masked to prefix length).
     */
    byte[] network();

    /**
     * CIDR prefix length (0-32 for IPv4, 0-128 for IPv6).
     */
    int prefixLength();

    /**
     * Per-rule hit counter. Incremented each time this rule matches.
     */
    LongAdder hitCounter();

    /**
     * Returns the current hit count.
     */
    default long hits() {
        return hitCounter().sum();
    }

    /**
     * Parse a CIDR string into network bytes and prefix length.
     * Supports both IPv4 (e.g., "192.168.1.0/24") and IPv6 (e.g., "2001:db8::/32").
     * A bare address without /prefix is treated as a host route (/32 or /128).
     */
    static CidrParts parseCidr(String cidr) {
        int slashIdx = cidr.indexOf('/');
        String addrPart;
        int prefix;
        if (slashIdx < 0) {
            addrPart = cidr;
            prefix = -1; // sentinel: resolve after parsing address
        } else {
            addrPart = cidr.substring(0, slashIdx);
            prefix = Integer.parseInt(cidr.substring(slashIdx + 1));
        }

        InetAddress addr;
        try {
            addr = InetAddress.getByName(addrPart);
        } catch (UnknownHostException e) {
            throw new IllegalArgumentException("Invalid IP address: " + addrPart, e);
        }

        byte[] raw = addr.getAddress();
        int maxPrefix = raw.length * 8;
        if (prefix < 0) {
            prefix = maxPrefix;
        }
        if (prefix > maxPrefix) {
            throw new IllegalArgumentException("Invalid prefix length " + prefix + " for " +
                    (raw.length == 4 ? "IPv4" : "IPv6") + " address");
        }

        // Mask the address to the prefix length
        byte[] masked = maskAddress(raw, prefix);
        return new CidrParts(masked, prefix);
    }

    /**
     * Mask an address byte array to the given prefix length.
     */
    static byte[] maskAddress(byte[] addr, int prefixLength) {
        byte[] result = addr.clone();
        int fullBytes = prefixLength / 8;
        int remainingBits = prefixLength % 8;

        if (fullBytes < result.length) {
            if (remainingBits > 0) {
                result[fullBytes] &= (byte) (0xFF << (8 - remainingBits));
                fullBytes++;
            }
            for (int i = fullBytes; i < result.length; i++) {
                result[i] = 0;
            }
        }
        return result;
    }

    /**
     * Create an Allow rule from a CIDR string.
     */
    static Allow allow(String cidr) {
        CidrParts parts = parseCidr(cidr);
        return new Allow(parts.network(), parts.prefixLength(), new LongAdder());
    }

    /**
     * Create a Deny rule from a CIDR string.
     */
    static Deny deny(String cidr) {
        CidrParts parts = parseCidr(cidr);
        return new Deny(parts.network(), parts.prefixLength(), new LongAdder());
    }

    record CidrParts(byte[] network, int prefixLength) {}

    record Allow(byte[] network, int prefixLength, LongAdder hitCounter) implements NACLRule {}

    record Deny(byte[] network, int prefixLength, LongAdder hitCounter) implements NACLRule {}
}
