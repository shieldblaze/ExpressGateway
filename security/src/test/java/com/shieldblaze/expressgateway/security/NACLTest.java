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

import io.netty.channel.ChannelHandler;
import io.netty.channel.embedded.EmbeddedChannel;
import io.netty.util.internal.SocketUtils;
import org.junit.jupiter.api.Test;

import java.net.SocketAddress;
import java.time.Duration;
import java.util.Collections;
import java.util.List;

import static org.junit.jupiter.api.Assertions.*;

class NACLTest {

    // ===== Backward compatibility: original rate limiting behavior =====

    @Test
    void globalRateLimitTest() {
        NACL nacl = new NACL(1, Duration.ofSeconds(10));

        EmbeddedChannel ch1 = newEmbeddedInetChannel("127.0.0.1", 5421, nacl);
        assertTrue(ch1.isActive());
        ch1.close();

        EmbeddedChannel ch2 = newEmbeddedInetChannel("127.0.0.1", 5421, nacl);
        assertFalse(ch2.isActive());
        ch2.close();
    }

    // ===== IPv4 CIDR Allowlist =====

    @Test
    void ipv4CidrAllowlist() {
        List<NACLRule> rules = List.of(
                NACLRule.allow("192.168.1.0/24"),
                NACLRule.allow("10.0.0.0/8")
        );

        NACL nacl = new NACL(0, null, 0, null,
                Collections.emptyList(), true,
                rules, NACL.Mode.ALLOWLIST, 1000);

        EmbeddedChannel ch1 = newEmbeddedInetChannel("192.168.1.50", 5000, nacl);
        assertTrue(ch1.isActive(), "192.168.1.50 should be allowed by /24 rule");
        ch1.close();

        EmbeddedChannel ch2 = newEmbeddedInetChannel("10.1.2.3", 5000, nacl);
        assertTrue(ch2.isActive(), "10.1.2.3 should be allowed by /8 rule");
        ch2.close();

        EmbeddedChannel ch3 = newEmbeddedInetChannel("172.16.0.1", 5000, nacl);
        assertFalse(ch3.isActive(), "172.16.0.1 should be denied in allowlist mode");
        ch3.close();
    }

    // ===== IPv4 CIDR Denylist =====

    @Test
    void ipv4CidrDenylist() {
        List<NACLRule> rules = List.of(
                NACLRule.deny("192.168.1.0/24")
        );

        NACL nacl = new NACL(0, null, 0, null,
                Collections.emptyList(), true,
                rules, NACL.Mode.DENYLIST, 1000);

        EmbeddedChannel ch1 = newEmbeddedInetChannel("192.168.1.100", 5000, nacl);
        assertFalse(ch1.isActive(), "192.168.1.100 should be denied by /24 deny rule");
        ch1.close();

        EmbeddedChannel ch2 = newEmbeddedInetChannel("10.0.0.1", 5000, nacl);
        assertTrue(ch2.isActive(), "10.0.0.1 should be allowed in denylist mode");
        ch2.close();
    }

    // ===== IPv6 CIDR matching =====

    @Test
    void ipv6CidrAllowlist() {
        List<NACLRule> rules = List.of(
                NACLRule.allow("2001:db8::/32")
        );

        NACL nacl = new NACL(0, null, 0, null,
                Collections.emptyList(), true,
                rules, NACL.Mode.ALLOWLIST, 1000);

        EmbeddedChannel ch1 = newEmbeddedInetChannel6("2001:db8::1", 5000, nacl);
        assertTrue(ch1.isActive(), "2001:db8::1 should be allowed by /32 rule");
        ch1.close();

        EmbeddedChannel ch2 = newEmbeddedInetChannel6("2001:db9::1", 5000, nacl);
        assertFalse(ch2.isActive(), "2001:db9::1 should be denied in allowlist mode");
        ch2.close();
    }

    // ===== Per-rule hit counters =====

    @Test
    void perRuleHitCounters() {
        NACLRule.Allow allowRule = NACLRule.allow("10.0.0.0/8");
        List<NACLRule> rules = List.of(allowRule);

        NACL nacl = new NACL(0, null, 0, null,
                Collections.emptyList(), true,
                rules, NACL.Mode.ALLOWLIST, 1000);

        EmbeddedChannel ch1 = newEmbeddedInetChannel("10.1.2.3", 5000, nacl);
        assertTrue(ch1.isActive());
        ch1.close();

        EmbeddedChannel ch2 = newEmbeddedInetChannel("10.4.5.6", 5000, nacl);
        assertTrue(ch2.isActive());
        ch2.close();

        assertTrue(allowRule.hits() >= 2, "Hit counter should reflect matched connections");
    }

    // ===== Dynamic rule updates =====

    @Test
    void dynamicRuleUpdate() {
        List<NACLRule> initialRules = List.of(NACLRule.allow("10.0.0.0/8"));

        NACL nacl = new NACL(0, null, 0, null,
                Collections.emptyList(), true,
                initialRules, NACL.Mode.ALLOWLIST, 1000);

        EmbeddedChannel ch1 = newEmbeddedInetChannel("10.1.2.3", 5000, nacl);
        assertTrue(ch1.isActive());
        ch1.close();

        nacl.updateRules(List.of(NACLRule.allow("192.168.0.0/16")));

        EmbeddedChannel ch2 = newEmbeddedInetChannel("10.1.2.3", 5000, nacl);
        assertFalse(ch2.isActive(), "10.1.2.3 should be denied after rule update");
        ch2.close();

        EmbeddedChannel ch3 = newEmbeddedInetChannel("192.168.1.1", 5000, nacl);
        assertTrue(ch3.isActive(), "192.168.1.1 should be allowed after rule update");
        ch3.close();
    }

    // ===== Mode switching =====

    @Test
    void modeSwitching() {
        List<NACLRule> rules = List.of(NACLRule.deny("192.168.1.0/24"));

        NACL nacl = new NACL(0, null, 0, null,
                Collections.emptyList(), true,
                rules, NACL.Mode.DENYLIST, 1000);

        EmbeddedChannel ch1 = newEmbeddedInetChannel("10.0.0.1", 5000, nacl);
        assertTrue(ch1.isActive());
        ch1.close();

        nacl.setMode(NACL.Mode.ALLOWLIST);

        EmbeddedChannel ch2 = newEmbeddedInetChannel("10.0.0.1", 5000, nacl);
        assertFalse(ch2.isActive(), "Should be denied after switching to allowlist mode");
        ch2.close();
    }

    // ===== Accepted/Denied counters =====

    @Test
    void acceptDenyCounters() {
        List<NACLRule> rules = List.of(NACLRule.allow("10.0.0.0/8"));

        NACL nacl = new NACL(0, null, 0, null,
                Collections.emptyList(), true,
                rules, NACL.Mode.ALLOWLIST, 1000);

        long baseDenied = nacl.totalDenied();
        long baseAccepted = nacl.totalAccepted();

        EmbeddedChannel ch1 = newEmbeddedInetChannel("10.0.0.1", 5000, nacl);
        ch1.close();

        EmbeddedChannel ch2 = newEmbeddedInetChannel("172.16.0.1", 5000, nacl);
        ch2.close();

        assertTrue(nacl.totalAccepted() >= baseAccepted + 1);
        assertTrue(nacl.totalDenied() >= baseDenied + 1);
    }

    // ===== Most specific prefix match =====

    @Test
    void longestPrefixMatchWins() {
        List<NACLRule> rules = List.of(
                NACLRule.allow("10.0.0.0/8"),
                NACLRule.deny("10.0.1.0/24")
        );

        NACL nacl = new NACL(0, null, 0, null,
                Collections.emptyList(), true,
                rules, NACL.Mode.DENYLIST, 1000);

        EmbeddedChannel ch1 = newEmbeddedInetChannel("10.0.0.1", 5000, nacl);
        assertTrue(ch1.isActive(), "10.0.0.1 should match /8 Allow and pass in denylist mode");
        ch1.close();

        EmbeddedChannel ch2 = newEmbeddedInetChannel("10.0.1.5", 5000, nacl);
        assertFalse(ch2.isActive(), "10.0.1.5 should match /24 Deny (longest prefix) and be denied");
        ch2.close();
    }

    // ===== Host route matching =====

    @Test
    void hostRouteMatching() {
        List<NACLRule> rules = List.of(
                NACLRule.allow("192.168.1.100")
        );

        NACL nacl = new NACL(0, null, 0, null,
                Collections.emptyList(), true,
                rules, NACL.Mode.ALLOWLIST, 1000);

        EmbeddedChannel ch1 = newEmbeddedInetChannel("192.168.1.100", 5000, nacl);
        assertTrue(ch1.isActive(), "Exact host match should be allowed");
        ch1.close();

        EmbeddedChannel ch2 = newEmbeddedInetChannel("192.168.1.101", 5000, nacl);
        assertFalse(ch2.isActive(), "Non-matching host should be denied");
        ch2.close();
    }

    // ===== NACLRule.parseCidr tests =====

    @Test
    void parseCidrIPv4() {
        NACLRule.CidrParts parts = NACLRule.parseCidr("192.168.1.0/24");
        assertEquals(24, parts.prefixLength());
        assertEquals(4, parts.network().length);
    }

    @Test
    void parseCidrIPv6() {
        NACLRule.CidrParts parts = NACLRule.parseCidr("2001:db8::/32");
        assertEquals(32, parts.prefixLength());
        assertEquals(16, parts.network().length);
    }

    @Test
    void parseCidrHostRoute() {
        NACLRule.CidrParts parts = NACLRule.parseCidr("10.0.0.1");
        assertEquals(32, parts.prefixLength());
    }

    @Test
    void parseCidrInvalidPrefix() {
        assertThrows(IllegalArgumentException.class, () -> NACLRule.parseCidr("10.0.0.0/33"));
    }

    // ===== Helper methods =====

    private static EmbeddedChannel newEmbeddedInetChannel(String ip, int port, ChannelHandler... handlers) {
        return new EmbeddedChannel(handlers) {
            @Override
            protected SocketAddress remoteAddress0() {
                return isActive() ? SocketUtils.socketAddress(ip, port) : null;
            }
        };
    }

    private static EmbeddedChannel newEmbeddedInetChannel6(String ip, int port, ChannelHandler... handlers) {
        return new EmbeddedChannel(handlers) {
            @Override
            protected SocketAddress remoteAddress0() {
                return isActive() ? SocketUtils.socketAddress(ip, port) : null;
            }
        };
    }
}
