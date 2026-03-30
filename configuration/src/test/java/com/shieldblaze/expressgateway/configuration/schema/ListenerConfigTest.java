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
package com.shieldblaze.expressgateway.configuration.schema;

import com.fasterxml.jackson.databind.ObjectMapper;
import org.junit.jupiter.api.Test;

import static org.junit.jupiter.api.Assertions.assertDoesNotThrow;
import static org.junit.jupiter.api.Assertions.assertEquals;
import static org.junit.jupiter.api.Assertions.assertFalse;
import static org.junit.jupiter.api.Assertions.assertThrows;
import static org.junit.jupiter.api.Assertions.assertTrue;

class ListenerConfigTest {

    private static final ObjectMapper MAPPER = new ObjectMapper();

    @Test
    void validTcpListener() {
        var config = new ListenerConfig("web", "0.0.0.0", 8080, "TCP", null, 10000, 60000);
        assertDoesNotThrow(config::validate);
    }

    @Test
    void validUdpListener() {
        var config = new ListenerConfig("dns", "0.0.0.0", 53, "UDP", null, 5000, 30000);
        assertDoesNotThrow(config::validate);
    }

    @Test
    void validQuicListenerWithTls() {
        var config = new ListenerConfig("quic-web", "0.0.0.0", 443, "QUIC", "my-cert", 10000, 30000);
        assertDoesNotThrow(config::validate);
    }

    @Test
    void quicWithoutTlsRejected() {
        var config = new ListenerConfig("quic-web", "0.0.0.0", 443, "QUIC", null, 10000, 30000);
        assertThrows(IllegalArgumentException.class, config::validate);
    }

    @Test
    void quicWithBlankTlsRejected() {
        var config = new ListenerConfig("quic-web", "0.0.0.0", 443, "QUIC", "   ", 10000, 30000);
        assertThrows(IllegalArgumentException.class, config::validate);
    }

    @Test
    void blankNameRejected() {
        var config = new ListenerConfig("", "0.0.0.0", 80, "TCP", null, 1000, 0);
        assertThrows(IllegalArgumentException.class, config::validate);
    }

    @Test
    void nullNameRejected() {
        var config = new ListenerConfig(null, "0.0.0.0", 80, "TCP", null, 1000, 0);
        assertThrows(NullPointerException.class, config::validate);
    }

    @Test
    void invalidBindAddressRejected() {
        var config = new ListenerConfig("web", "not-an-ip-$$$", 80, "TCP", null, 1000, 0);
        assertThrows(IllegalArgumentException.class, config::validate);
    }

    @Test
    void hostnameRejectedNoDns() {
        // Hostnames should be rejected because we validate as IP literals only
        var config = new ListenerConfig("web", "example.com", 80, "TCP", null, 1000, 0);
        assertThrows(IllegalArgumentException.class, config::validate);
    }

    @Test
    void localhostHostnameRejected() {
        // Even "localhost" should be rejected (it's not an IP literal)
        var config = new ListenerConfig("web", "localhost", 80, "TCP", null, 1000, 0);
        assertThrows(IllegalArgumentException.class, config::validate);
    }

    @Test
    void validIpv4Accepted() {
        var config = new ListenerConfig("web", "192.168.1.1", 80, "TCP", null, 1000, 0);
        assertDoesNotThrow(config::validate);
    }

    @Test
    void validIpv4AllZeros() {
        var config = new ListenerConfig("web", "0.0.0.0", 80, "TCP", null, 1000, 0);
        assertDoesNotThrow(config::validate);
    }

    @Test
    void validIpv4AllMax() {
        var config = new ListenerConfig("web", "255.255.255.255", 80, "TCP", null, 1000, 0);
        assertDoesNotThrow(config::validate);
    }

    @Test
    void portZeroRejected() {
        var config = new ListenerConfig("web", "0.0.0.0", 0, "TCP", null, 1000, 0);
        assertThrows(IllegalArgumentException.class, config::validate);
    }

    @Test
    void portAbove65535Rejected() {
        var config = new ListenerConfig("web", "0.0.0.0", 70000, "TCP", null, 1000, 0);
        assertThrows(IllegalArgumentException.class, config::validate);
    }

    @Test
    void invalidProtocolRejected() {
        var config = new ListenerConfig("web", "0.0.0.0", 80, "HTTP", null, 1000, 0);
        assertThrows(IllegalArgumentException.class, config::validate);
    }

    @Test
    void zeroMaxConnectionsRejected() {
        var config = new ListenerConfig("web", "0.0.0.0", 80, "TCP", null, 0, 0);
        assertThrows(IllegalArgumentException.class, config::validate);
    }

    @Test
    void negativeIdleTimeoutRejected() {
        var config = new ListenerConfig("web", "0.0.0.0", 80, "TCP", null, 1000, -1);
        assertThrows(IllegalArgumentException.class, config::validate);
    }

    @Test
    void zeroIdleTimeoutAccepted() {
        var config = new ListenerConfig("web", "0.0.0.0", 80, "TCP", null, 1000, 0);
        assertDoesNotThrow(config::validate);
    }

    @Test
    void ipv6AddressAccepted() {
        var config = new ListenerConfig("web", "::1", 80, "TCP", null, 1000, 0);
        assertDoesNotThrow(config::validate);
    }

    @Test
    void ipv6FullAddressAccepted() {
        var config = new ListenerConfig("web", "2001:0db8:85a3:0000:0000:8a2e:0370:7334", 80, "TCP", null, 1000, 0);
        assertDoesNotThrow(config::validate);
    }

    @Test
    void ipv6AllZerosAccepted() {
        var config = new ListenerConfig("web", "::", 80, "TCP", null, 1000, 0);
        assertDoesNotThrow(config::validate);
    }

    @Test
    void ipLiteralValidation() {
        // Valid IPv4
        assertTrue(ListenerConfig.isValidIpLiteral("0.0.0.0"));
        assertTrue(ListenerConfig.isValidIpLiteral("127.0.0.1"));
        assertTrue(ListenerConfig.isValidIpLiteral("255.255.255.255"));
        assertTrue(ListenerConfig.isValidIpLiteral("10.0.0.1"));

        // Valid IPv6
        assertTrue(ListenerConfig.isValidIpLiteral("::1"));
        assertTrue(ListenerConfig.isValidIpLiteral("::"));
        assertTrue(ListenerConfig.isValidIpLiteral("fe80::1"));

        // Invalid: hostnames
        assertFalse(ListenerConfig.isValidIpLiteral("example.com"));
        assertFalse(ListenerConfig.isValidIpLiteral("localhost"));
        assertFalse(ListenerConfig.isValidIpLiteral("my-host"));

        // Invalid: garbage
        assertFalse(ListenerConfig.isValidIpLiteral("not-an-ip"));
        assertFalse(ListenerConfig.isValidIpLiteral(""));
        assertFalse(ListenerConfig.isValidIpLiteral("256.0.0.1"));
    }

    @Test
    void jsonRoundTrip() throws Exception {
        var original = new ListenerConfig("web", "0.0.0.0", 8080, "TCP", "my-cert", 10000, 60000);
        String json = MAPPER.writeValueAsString(original);
        ListenerConfig deserialized = MAPPER.readValue(json, ListenerConfig.class);
        assertEquals(original, deserialized);
    }
}
