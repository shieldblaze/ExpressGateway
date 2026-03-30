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

import com.fasterxml.jackson.annotation.JsonProperty;

import java.util.Objects;
import java.util.Set;
import java.util.regex.Pattern;

/**
 * Configuration schema for a network listener (frontend bind point).
 *
 * @param name            The listener name (must be non-blank)
 * @param bindAddress     The address to bind to (e.g. "0.0.0.0", "::1")
 * @param port            The port to bind to (1-65535)
 * @param protocol        The transport protocol ("TCP", "UDP", "QUIC")
 * @param tlsConfigRef    Optional reference to a TLS configuration by name (null if no TLS)
 * @param maxConnections  Maximum concurrent connections (must be >= 1)
 * @param idleTimeoutMs   Idle timeout in milliseconds before closing inactive connections (must be >= 0)
 */
public record ListenerConfig(
        @JsonProperty("name") String name,
        @JsonProperty("bindAddress") String bindAddress,
        @JsonProperty("port") int port,
        @JsonProperty("protocol") String protocol,
        @JsonProperty("tlsConfigRef") String tlsConfigRef,
        @JsonProperty("maxConnections") int maxConnections,
        @JsonProperty("idleTimeoutMs") long idleTimeoutMs
) {

    private static final Set<String> VALID_PROTOCOLS = Set.of("TCP", "UDP", "QUIC");

    /**
     * Pattern for valid IPv4 literal addresses.
     * Matches: 0.0.0.0, 127.0.0.1, 255.255.255.255, etc.
     */
    private static final Pattern IPV4_PATTERN = Pattern.compile(
            "^((25[0-5]|2[0-4]\\d|[01]?\\d\\d?)\\.){3}(25[0-5]|2[0-4]\\d|[01]?\\d\\d?)$");

    /**
     * Pattern for valid IPv6 literal addresses.
     * Matches standard forms including: ::1, ::, fe80::1, 2001:db8::1,
     * fully expanded addresses, and mixed IPv4-in-IPv6 (e.g. ::ffff:192.0.2.1).
     */
    private static final Pattern IPV6_PATTERN = Pattern.compile(
            "^(" +
            // Full form or leading groups with :: shorthand
            "([0-9a-fA-F]{1,4}:){7}[0-9a-fA-F]{1,4}" +                   // 1:2:3:4:5:6:7:8
            "|([0-9a-fA-F]{1,4}:){1,7}:" +                                // 1::  through 1:2:3:4:5:6:7::
            "|([0-9a-fA-F]{1,4}:){1,6}:[0-9a-fA-F]{1,4}" +               // 1::8  through 1:2:3:4:5:6::8
            "|([0-9a-fA-F]{1,4}:){1,5}(:[0-9a-fA-F]{1,4}){1,2}" +        // 1::7:8  through 1:2:3:4:5::7:8
            "|([0-9a-fA-F]{1,4}:){1,4}(:[0-9a-fA-F]{1,4}){1,3}" +        // 1::6:7:8  through 1:2:3:4::6:7:8
            "|([0-9a-fA-F]{1,4}:){1,3}(:[0-9a-fA-F]{1,4}){1,4}" +        // 1::5:6:7:8
            "|([0-9a-fA-F]{1,4}:){1,2}(:[0-9a-fA-F]{1,4}){1,5}" +        // 1::4:5:6:7:8
            "|[0-9a-fA-F]{1,4}:(:[0-9a-fA-F]{1,4}){1,6}" +               // 1::3:4:5:6:7:8
            "|:(:[0-9a-fA-F]{1,4}){1,7}" +                                // ::2:3:4:5:6:7:8
            "|::" +                                                         // :: (all zeros)
            // IPv4-mapped/compatible
            "|([0-9a-fA-F]{1,4}:){1,5}:((25[0-5]|2[0-4]\\d|[01]?\\d\\d?)\\.){3}(25[0-5]|2[0-4]\\d|[01]?\\d\\d?)" +
            "|::([fF]{4}:)?((25[0-5]|2[0-4]\\d|[01]?\\d\\d?)\\.){3}(25[0-5]|2[0-4]\\d|[01]?\\d\\d?)" +
            ")$");

    /**
     * Validate all fields for correctness.
     * Uses regex to validate bind addresses as IP literals without performing DNS resolution.
     *
     * @throws IllegalArgumentException if any field is invalid
     */
    public void validate() {
        Objects.requireNonNull(name, "name");
        if (name.isBlank()) {
            throw new IllegalArgumentException("name must not be blank");
        }
        Objects.requireNonNull(bindAddress, "bindAddress");
        if (bindAddress.isBlank()) {
            throw new IllegalArgumentException("bindAddress must not be blank");
        }
        if (!isValidIpLiteral(bindAddress)) {
            throw new IllegalArgumentException("bindAddress is not a valid IP address literal: " + bindAddress);
        }
        if (port < 1 || port > 65535) {
            throw new IllegalArgumentException("port must be in range [1, 65535], got: " + port);
        }
        Objects.requireNonNull(protocol, "protocol");
        if (!VALID_PROTOCOLS.contains(protocol)) {
            throw new IllegalArgumentException("protocol must be one of " + VALID_PROTOCOLS + ", got: " + protocol);
        }
        // QUIC mandates TLS (RFC 9001)
        if ("QUIC".equals(protocol) && (tlsConfigRef == null || tlsConfigRef.isBlank())) {
            throw new IllegalArgumentException("QUIC protocol requires a tlsConfigRef (TLS is mandatory per RFC 9001)");
        }
        if (maxConnections < 1) {
            throw new IllegalArgumentException("maxConnections must be >= 1, got: " + maxConnections);
        }
        if (idleTimeoutMs < 0) {
            throw new IllegalArgumentException("idleTimeoutMs must be >= 0, got: " + idleTimeoutMs);
        }
    }

    /**
     * Checks if the given string is a valid IPv4 or IPv6 literal address
     * without performing any DNS resolution.
     */
    static boolean isValidIpLiteral(String address) {
        return IPV4_PATTERN.matcher(address).matches() || IPV6_PATTERN.matcher(address).matches();
    }
}
