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
 * Sealed interface for protocol-to-protocol translation between HTTP/1.1, HTTP/2, and HTTP/3.
 *
 * <p>Each permitted implementation handles a specific directional translation:
 * <ul>
 *   <li>{@link H1ToH2Translator} -- HTTP/1.1 → HTTP/2 (RFC 9112 → RFC 9113)</li>
 *   <li>{@link H1ToH3Translator} -- HTTP/1.1 → HTTP/3 (RFC 9112 → RFC 9114)</li>
 *   <li>{@link H2ToH1Translator} -- HTTP/2 → HTTP/1.1 (RFC 9113 → RFC 9112)</li>
 *   <li>{@link H2ToH3Translator} -- HTTP/2 → HTTP/3 (RFC 9113 → RFC 9114)</li>
 *   <li>{@link H3ToH1Translator} -- HTTP/3 → HTTP/1.1 (RFC 9114 → RFC 9112)</li>
 *   <li>{@link H3ToH2Translator} -- HTTP/3 → HTTP/2 (RFC 9114 → RFC 9113)</li>
 * </ul>
 *
 * <p>The sealed hierarchy enables exhaustive pattern matching with Java 21 switch
 * expressions, ensuring compile-time completeness when dispatching on translator type.</p>
 */
public sealed interface ProtocolTranslator
        permits H1ToH2Translator, H1ToH3Translator,
                H2ToH1Translator, H2ToH3Translator,
                H3ToH1Translator, H3ToH2Translator {

    /**
     * Returns the source protocol version this translator reads from.
     */
    ProtocolVersion sourceProtocol();

    /**
     * Returns the target protocol version this translator writes to.
     */
    ProtocolVersion targetProtocol();

    /**
     * Factory method to obtain the appropriate translator for a given source/target pair.
     *
     * @param source the protocol version of the inbound message
     * @param target the protocol version of the outbound connection
     * @return the translator for this protocol pair
     * @throws IllegalArgumentException if source equals target (no translation needed)
     *                                  or if the combination is unsupported
     */
    static ProtocolTranslator forPair(ProtocolVersion source, ProtocolVersion target) {
        return switch (source) {
            case HTTP_1_1 -> switch (target) {
                case HTTP_2 -> H1ToH2Translator.INSTANCE;
                case HTTP_3 -> H1ToH3Translator.INSTANCE;
                case HTTP_1_1 -> throw new IllegalArgumentException("No translation needed for same protocol: HTTP/1.1");
            };
            case HTTP_2 -> switch (target) {
                case HTTP_1_1 -> H2ToH1Translator.INSTANCE;
                case HTTP_3 -> H2ToH3Translator.INSTANCE;
                case HTTP_2 -> throw new IllegalArgumentException("No translation needed for same protocol: HTTP/2");
            };
            case HTTP_3 -> switch (target) {
                case HTTP_1_1 -> H3ToH1Translator.INSTANCE;
                case HTTP_2 -> H3ToH2Translator.INSTANCE;
                case HTTP_3 -> throw new IllegalArgumentException("No translation needed for same protocol: HTTP/3");
            };
        };
    }
}
