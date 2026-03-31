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

import java.util.ArrayList;
import java.util.List;

/**
 * A binary radix trie (Patricia trie) for longest-prefix matching of IP addresses.
 * <p>
 * Each node represents a bit position in the IP address. Walking the trie from root
 * to leaf follows the bits of the address being looked up, and the deepest node
 * with an attached rule is the longest (most specific) matching prefix.
 * <p>
 * This gives O(prefix_length) lookup time -- at most 32 steps for IPv4, 128 for IPv6 --
 * instead of O(n) linear scan over all rules.
 * <p>
 * Thread safety: This class is NOT thread-safe. The caller ({@link NACL}) achieves
 * thread safety via copy-on-write: a new trie is built and swapped atomically on
 * rule updates, while reads use the current immutable reference without locking.
 */
final class IpRadixTrie {

    private static final class Node {
        Node zero;    // bit = 0
        Node one;     // bit = 1
        NACLRule rule; // null if this node is not a prefix endpoint
    }

    private final Node root = new Node();

    /**
     * Insert a rule into the trie.
     */
    void insert(NACLRule rule) {
        byte[] network = rule.network();
        int prefixLen = rule.prefixLength();
        Node current = root;

        for (int i = 0; i < prefixLen; i++) {
            int bit = getBit(network, i);
            if (bit == 0) {
                if (current.zero == null) {
                    current.zero = new Node();
                }
                current = current.zero;
            } else {
                if (current.one == null) {
                    current.one = new Node();
                }
                current = current.one;
            }
        }
        current.rule = rule;
    }

    /**
     * Find the longest-prefix matching rule for the given address.
     *
     * @param address raw IP address bytes (4 for IPv4, 16 for IPv6)
     * @return the most specific matching rule, or null if no match
     */
    NACLRule longestPrefixMatch(byte[] address) {
        Node current = root;
        NACLRule bestMatch = root.rule; // default route if present
        int maxBits = address.length * 8;

        for (int i = 0; i < maxBits && current != null; i++) {
            int bit = getBit(address, i);
            current = (bit == 0) ? current.zero : current.one;
            if (current != null && current.rule != null) {
                bestMatch = current.rule;
            }
        }
        return bestMatch;
    }

    /**
     * Collect all rules in this trie (for snapshot/iteration).
     */
    List<NACLRule> allRules() {
        List<NACLRule> result = new ArrayList<>();
        collectRules(root, result);
        return result;
    }

    private void collectRules(Node node, List<NACLRule> result) {
        if (node == null) return;
        if (node.rule != null) {
            result.add(node.rule);
        }
        collectRules(node.zero, result);
        collectRules(node.one, result);
    }

    /**
     * Get the bit at position {@code bitIndex} from a byte array.
     * Bit 0 is the MSB of byte[0], bit 7 is the LSB of byte[0], etc.
     */
    private static int getBit(byte[] data, int bitIndex) {
        int byteIndex = bitIndex / 8;
        int bitOffset = 7 - (bitIndex % 8);
        return (data[byteIndex] >> bitOffset) & 1;
    }
}
