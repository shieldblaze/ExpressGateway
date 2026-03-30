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
package com.shieldblaze.expressgateway.configuration.quic;

import com.fasterxml.jackson.annotation.JsonIgnore;
import com.fasterxml.jackson.annotation.JsonProperty;
import com.shieldblaze.expressgateway.common.utils.NumberUtil;
import com.shieldblaze.expressgateway.configuration.Configuration;

/**
 * Configuration for QUIC transport parameters (RFC 9000).
 *
 * <p>This configuration owns the QUIC transport-level settings that are independent
 * of the application protocol running over QUIC. HTTP/3 (RFC 9114), WebTransport,
 * or any other QUIC-based protocol can use these transport parameters.</p>
 *
 * <p>QUIC transport parameters are advertised during the handshake (RFC 9000 Section 18)
 * and constrain the peer's behavior for the lifetime of the connection.</p>
 */
public final class QuicConfiguration implements Configuration<QuicConfiguration> {

    /**
     * Whether QUIC transport is enabled. When disabled, no QUIC listener is started.
     */
    @JsonProperty
    private boolean enabled;

    /**
     * QUIC max idle timeout in milliseconds (RFC 9000 Section 10.1).
     * Connection is closed if no packets exchanged within this period.
     */
    @JsonProperty
    private long maxIdleTimeoutMs;

    /**
     * QUIC initial max data (connection-level flow control) in bytes.
     * RFC 9000 Section 4.1 — limits aggregate data across all streams.
     */
    @JsonProperty
    private long initialMaxData;

    /**
     * QUIC initial max stream data for bidirectional local streams in bytes.
     * RFC 9000 Section 4.1 — per-stream flow control window.
     */
    @JsonProperty
    private long initialMaxStreamDataBidiLocal;

    /**
     * QUIC initial max stream data for bidirectional remote streams in bytes.
     */
    @JsonProperty
    private long initialMaxStreamDataBidiRemote;

    /**
     * QUIC initial max stream data for unidirectional streams in bytes.
     * Required for HTTP/3 control streams (RFC 9114 Section 6.2).
     */
    @JsonProperty
    private long initialMaxStreamDataUni;

    /**
     * Maximum concurrent bidirectional streams (RFC 9000 Section 4.6).
     * Maps to application-level request concurrency limit.
     */
    @JsonProperty
    private long initialMaxStreamsBidi;

    /**
     * Maximum concurrent unidirectional streams (RFC 9000 Section 4.6).
     * HTTP/3 requires at least 3 for control, QPACK encoder, and QPACK decoder.
     */
    @JsonProperty
    private long initialMaxStreamsUni;

    /**
     * Maximum QUIC connections to pool per backend Node.
     * Each QUIC connection supports multiplexed streams.
     */
    @JsonProperty
    private int maxConnectionsPerNode;

    /**
     * UDP port for QUIC listener. May differ from the TCP port
     * used by HTTP/1.1 and HTTP/2.
     */
    @JsonProperty
    private int port;

    /**
     * Whether to enable 0-RTT (early data) for QUIC connections.
     * RFC 9001 Section 4.6 — enables sending data during the handshake
     * at the cost of replay vulnerability for non-idempotent requests.
     */
    @JsonProperty
    private boolean zeroRttEnabled;

    /**
     * Graceful shutdown drain timeout in milliseconds for QUIC connections.
     * When closing, GOAWAY is sent and close is delayed by this duration.
     */
    @JsonProperty
    private long gracefulShutdownDrainMs;

    /**
     * Idle timeout in seconds for QUIC proxy sessions (L4 UDP forwarding mode).
     * When no packets are exchanged within this period, the proxy session mapping
     * is removed and the backend channel is closed. Distinct from
     * {@link #maxIdleTimeoutMs} which is a QUIC transport parameter advertised
     * to the peer during the handshake.
     *
     * <p>Default: 30 seconds, aligning with QUIC max_idle_timeout defaults.</p>
     */
    @JsonProperty
    private long quicProxySessionIdleTimeoutSeconds = 30;

    /**
     * Whether CID-based routing is enabled for QUIC proxy sessions.
     * When enabled, incoming QUIC datagrams are routed to backend sessions
     * based on the Destination Connection ID (DCID) extracted from the packet
     * header, supporting QUIC connection migration (RFC 9000 Section 9).
     * When disabled, routing falls back to source address only.
     *
     * <p>Default: true.</p>
     */
    @JsonProperty
    private boolean cidBasedRoutingEnabled = true;

    @JsonIgnore
    private boolean validated;

    public static final QuicConfiguration DEFAULT = new QuicConfiguration();

    static {
        DEFAULT.enabled = true;
        DEFAULT.maxIdleTimeoutMs = 30_000;
        DEFAULT.initialMaxData = 10_000_000;
        DEFAULT.initialMaxStreamDataBidiLocal = 1_000_000;
        DEFAULT.initialMaxStreamDataBidiRemote = 1_000_000;
        DEFAULT.initialMaxStreamDataUni = 1_000_000;
        DEFAULT.initialMaxStreamsBidi = 100;
        DEFAULT.initialMaxStreamsUni = 3;
        DEFAULT.maxConnectionsPerNode = 4;
        DEFAULT.port = 443;
        DEFAULT.zeroRttEnabled = false;
        DEFAULT.gracefulShutdownDrainMs = 5000;
        DEFAULT.quicProxySessionIdleTimeoutSeconds = 30;
        DEFAULT.cidBasedRoutingEnabled = true;
        DEFAULT.validated = true;
    }

    QuicConfiguration() {
        // Prevent outside initialization
    }

    public boolean enabled() {
        assertValidated();
        return enabled;
    }

    public QuicConfiguration setEnabled(boolean enabled) {
        this.enabled = enabled;
        return this;
    }

    public long maxIdleTimeoutMs() {
        assertValidated();
        return maxIdleTimeoutMs;
    }

    public QuicConfiguration setMaxIdleTimeoutMs(long maxIdleTimeoutMs) {
        this.maxIdleTimeoutMs = maxIdleTimeoutMs;
        return this;
    }

    public long initialMaxData() {
        assertValidated();
        return initialMaxData;
    }

    public QuicConfiguration setInitialMaxData(long initialMaxData) {
        this.initialMaxData = initialMaxData;
        return this;
    }

    public long initialMaxStreamDataBidiLocal() {
        assertValidated();
        return initialMaxStreamDataBidiLocal;
    }

    public QuicConfiguration setInitialMaxStreamDataBidiLocal(long initialMaxStreamDataBidiLocal) {
        this.initialMaxStreamDataBidiLocal = initialMaxStreamDataBidiLocal;
        return this;
    }

    public long initialMaxStreamDataBidiRemote() {
        assertValidated();
        return initialMaxStreamDataBidiRemote;
    }

    public QuicConfiguration setInitialMaxStreamDataBidiRemote(long initialMaxStreamDataBidiRemote) {
        this.initialMaxStreamDataBidiRemote = initialMaxStreamDataBidiRemote;
        return this;
    }

    public long initialMaxStreamDataUni() {
        assertValidated();
        return initialMaxStreamDataUni;
    }

    public QuicConfiguration setInitialMaxStreamDataUni(long initialMaxStreamDataUni) {
        this.initialMaxStreamDataUni = initialMaxStreamDataUni;
        return this;
    }

    public long initialMaxStreamsBidi() {
        assertValidated();
        return initialMaxStreamsBidi;
    }

    public QuicConfiguration setInitialMaxStreamsBidi(long initialMaxStreamsBidi) {
        this.initialMaxStreamsBidi = initialMaxStreamsBidi;
        return this;
    }

    public long initialMaxStreamsUni() {
        assertValidated();
        return initialMaxStreamsUni;
    }

    public QuicConfiguration setInitialMaxStreamsUni(long initialMaxStreamsUni) {
        this.initialMaxStreamsUni = initialMaxStreamsUni;
        return this;
    }

    public int maxConnectionsPerNode() {
        assertValidated();
        return maxConnectionsPerNode;
    }

    public QuicConfiguration setMaxConnectionsPerNode(int maxConnectionsPerNode) {
        this.maxConnectionsPerNode = maxConnectionsPerNode;
        return this;
    }

    public int port() {
        assertValidated();
        return port;
    }

    public QuicConfiguration setPort(int port) {
        this.port = port;
        return this;
    }

    public boolean zeroRttEnabled() {
        assertValidated();
        return zeroRttEnabled;
    }

    public QuicConfiguration setZeroRttEnabled(boolean zeroRttEnabled) {
        this.zeroRttEnabled = zeroRttEnabled;
        return this;
    }

    public long gracefulShutdownDrainMs() {
        assertValidated();
        return gracefulShutdownDrainMs;
    }

    public QuicConfiguration setGracefulShutdownDrainMs(long gracefulShutdownDrainMs) {
        this.gracefulShutdownDrainMs = gracefulShutdownDrainMs;
        return this;
    }

    public long quicProxySessionIdleTimeoutSeconds() {
        assertValidated();
        return quicProxySessionIdleTimeoutSeconds;
    }

    public QuicConfiguration setQuicProxySessionIdleTimeoutSeconds(long quicProxySessionIdleTimeoutSeconds) {
        this.quicProxySessionIdleTimeoutSeconds = quicProxySessionIdleTimeoutSeconds;
        return this;
    }

    public boolean cidBasedRoutingEnabled() {
        assertValidated();
        return cidBasedRoutingEnabled;
    }

    public QuicConfiguration setCidBasedRoutingEnabled(boolean cidBasedRoutingEnabled) {
        this.cidBasedRoutingEnabled = cidBasedRoutingEnabled;
        return this;
    }

    @Override
    public QuicConfiguration validate() {
        NumberUtil.checkZeroOrPositive(maxIdleTimeoutMs, "MaxIdleTimeoutMs");
        NumberUtil.checkPositive(initialMaxData, "InitialMaxData");
        NumberUtil.checkPositive(initialMaxStreamDataBidiLocal, "InitialMaxStreamDataBidiLocal");
        NumberUtil.checkPositive(initialMaxStreamDataBidiRemote, "InitialMaxStreamDataBidiRemote");
        NumberUtil.checkPositive(initialMaxStreamDataUni, "InitialMaxStreamDataUni");
        NumberUtil.checkPositive(initialMaxStreamsBidi, "InitialMaxStreamsBidi");
        NumberUtil.checkPositive(initialMaxStreamsUni, "InitialMaxStreamsUni");
        NumberUtil.checkPositive(maxConnectionsPerNode, "MaxConnectionsPerNode");
        NumberUtil.checkPositive(port, "Port");
        NumberUtil.checkZeroOrPositive(gracefulShutdownDrainMs, "GracefulShutdownDrainMs");
        NumberUtil.checkPositive(quicProxySessionIdleTimeoutSeconds, "QuicProxySessionIdleTimeoutSeconds");
        validated = true;
        return this;
    }

    @Override
    public boolean validated() {
        return validated;
    }
}
