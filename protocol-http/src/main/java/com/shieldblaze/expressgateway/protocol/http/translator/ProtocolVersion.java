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
package com.shieldblaze.expressgateway.protocol.http.translator;

/**
 * Enumerates the HTTP protocol versions supported by the translation layer.
 *
 * <p>Each version maps to specific framing, header encoding, and flow control
 * semantics that the translators must account for when converting between protocols.</p>
 */
public enum ProtocolVersion {

    /**
     * HTTP/1.1 per RFC 9112. Text-based framing, serial request/response on TCP,
     * chunked transfer encoding for streaming, hop-by-hop Connection headers.
     */
    HTTP_1_1("1.1"),

    /**
     * HTTP/2 per RFC 9113. Binary framing over TCP+TLS, multiplexed streams,
     * HPACK header compression, connection+stream-level flow control.
     */
    HTTP_2("2.0"),

    /**
     * HTTP/3 per RFC 9114. Binary framing over QUIC (UDP), independent streams
     * without head-of-line blocking, QPACK header compression, QUIC-level
     * flow control (stream + connection).
     */
    HTTP_3("3.0");

    private final String viaToken;

    ProtocolVersion(String viaToken) {
        this.viaToken = viaToken;
    }

    /**
     * Returns the protocol version token for the Via header per RFC 9110 Section 7.6.3.
     * Examples: "1.1", "2.0", "3.0".
     */
    public String viaToken() {
        return viaToken;
    }
}
