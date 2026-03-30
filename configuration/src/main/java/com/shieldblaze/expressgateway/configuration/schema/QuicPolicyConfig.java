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

/**
 * Configuration schema for QUIC transport parameters (RFC 9000 Section 18).
 *
 * <p>These parameters are advertised during the QUIC handshake and constrain
 * the peer's behavior for the lifetime of the connection.</p>
 *
 * @param maxIdleTimeoutMs                    Max idle timeout in ms (RFC 9000 Section 10.1); 0 means default
 * @param maxUdpPayloadSize                   Maximum UDP payload size in bytes (RFC 9000 Section 18.2); must be >= 1200
 * @param initialMaxData                      Connection-level flow control limit in bytes (RFC 9000 Section 4.1)
 * @param initialMaxStreamDataBidiLocal       Per-stream flow control for locally-initiated bidi streams in bytes
 * @param initialMaxStreamDataBidiRemote      Per-stream flow control for remotely-initiated bidi streams in bytes
 * @param initialMaxStreamDataUni             Per-stream flow control for unidirectional streams in bytes
 * @param initialMaxStreamsBidi               Max concurrent bidirectional streams (RFC 9000 Section 4.6)
 * @param initialMaxStreamsUni                 Max concurrent unidirectional streams; HTTP/3 needs >= 3
 * @param ackDelayExponent                    ACK delay exponent (RFC 9000 Section 18.2); must be in [0, 20]
 * @param maxAckDelayMs                       Maximum ACK delay in ms (RFC 9000 Section 18.2); must be < 2^14
 * @param disableActiveMigration              Whether to disable active connection migration (RFC 9000 Section 18.2)
 * @param activeConnectionIdLimit             Max active connection IDs (RFC 9000 Section 18.2); must be >= 2
 */
public record QuicPolicyConfig(
        @JsonProperty("maxIdleTimeoutMs") long maxIdleTimeoutMs,
        @JsonProperty("maxUdpPayloadSize") int maxUdpPayloadSize,
        @JsonProperty("initialMaxData") long initialMaxData,
        @JsonProperty("initialMaxStreamDataBidiLocal") long initialMaxStreamDataBidiLocal,
        @JsonProperty("initialMaxStreamDataBidiRemote") long initialMaxStreamDataBidiRemote,
        @JsonProperty("initialMaxStreamDataUni") long initialMaxStreamDataUni,
        @JsonProperty("initialMaxStreamsBidi") long initialMaxStreamsBidi,
        @JsonProperty("initialMaxStreamsUni") long initialMaxStreamsUni,
        @JsonProperty("ackDelayExponent") int ackDelayExponent,
        @JsonProperty("maxAckDelayMs") long maxAckDelayMs,
        @JsonProperty("disableActiveMigration") boolean disableActiveMigration,
        @JsonProperty("activeConnectionIdLimit") int activeConnectionIdLimit
) {

    /** RFC 9000 Section 18.2: minimum UDP payload size is 1200 bytes */
    private static final int MIN_UDP_PAYLOAD_SIZE = 1200;
    /** RFC 9000 Section 18.2: maximum UDP payload size is 65527 bytes (65535 - 8 byte UDP header) */
    private static final int MAX_UDP_PAYLOAD_SIZE = 65527;
    /** RFC 9000 Section 18.2: max_ack_delay must be less than 2^14 ms */
    private static final long MAX_ACK_DELAY_UPPER_BOUND = 16384;
    /** RFC 9000 Section 18.2: ack_delay_exponent must not exceed 20 */
    private static final int MAX_ACK_DELAY_EXPONENT = 20;

    /**
     * Validate all fields for correctness per RFC 9000 constraints.
     *
     * @throws IllegalArgumentException if any field violates RFC 9000 constraints
     */
    public void validate() {
        if (maxIdleTimeoutMs < 0) {
            throw new IllegalArgumentException("maxIdleTimeoutMs must be >= 0, got: " + maxIdleTimeoutMs);
        }
        if (maxUdpPayloadSize < MIN_UDP_PAYLOAD_SIZE) {
            throw new IllegalArgumentException(
                    "maxUdpPayloadSize must be >= " + MIN_UDP_PAYLOAD_SIZE + " (RFC 9000 Section 18.2), got: " + maxUdpPayloadSize);
        }
        if (maxUdpPayloadSize > MAX_UDP_PAYLOAD_SIZE) {
            throw new IllegalArgumentException(
                    "maxUdpPayloadSize must be <= " + MAX_UDP_PAYLOAD_SIZE + " (RFC 9000 Section 18.2), got: " + maxUdpPayloadSize);
        }
        if (initialMaxData < 0) {
            throw new IllegalArgumentException("initialMaxData must be >= 0, got: " + initialMaxData);
        }
        if (initialMaxData == 0) {
            // Log a warning at validation time -- initialMaxData == 0 effectively disables
            // connection-level flow control, which will prevent data transfer.
            org.apache.logging.log4j.LogManager.getLogger(QuicPolicyConfig.class)
                    .warn("initialMaxData is 0, which disables connection-level flow control. " +
                          "No data can be sent on this connection until a MAX_DATA frame is received.");
        }
        if (initialMaxStreamDataBidiLocal < 0) {
            throw new IllegalArgumentException("initialMaxStreamDataBidiLocal must be >= 0, got: " + initialMaxStreamDataBidiLocal);
        }
        if (initialMaxStreamDataBidiRemote < 0) {
            throw new IllegalArgumentException("initialMaxStreamDataBidiRemote must be >= 0, got: " + initialMaxStreamDataBidiRemote);
        }
        if (initialMaxStreamDataUni < 0) {
            throw new IllegalArgumentException("initialMaxStreamDataUni must be >= 0, got: " + initialMaxStreamDataUni);
        }
        if (initialMaxStreamsBidi < 0) {
            throw new IllegalArgumentException("initialMaxStreamsBidi must be >= 0, got: " + initialMaxStreamsBidi);
        }
        if (initialMaxStreamsUni < 0) {
            throw new IllegalArgumentException("initialMaxStreamsUni must be >= 0, got: " + initialMaxStreamsUni);
        }
        if (ackDelayExponent < 0 || ackDelayExponent > MAX_ACK_DELAY_EXPONENT) {
            throw new IllegalArgumentException(
                    "ackDelayExponent must be in range [0, " + MAX_ACK_DELAY_EXPONENT + "] (RFC 9000 Section 18.2), got: " + ackDelayExponent);
        }
        if (maxAckDelayMs < 0 || maxAckDelayMs >= MAX_ACK_DELAY_UPPER_BOUND) {
            throw new IllegalArgumentException(
                    "maxAckDelayMs must be in range [0, " + (MAX_ACK_DELAY_UPPER_BOUND - 1) + "] (RFC 9000 Section 18.2), got: " + maxAckDelayMs);
        }
        if (activeConnectionIdLimit < 2) {
            throw new IllegalArgumentException(
                    "activeConnectionIdLimit must be >= 2 (RFC 9000 Section 18.2), got: " + activeConnectionIdLimit);
        }
    }
}
