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
package com.shieldblaze.expressgateway.configuration.http3;

import com.fasterxml.jackson.annotation.JsonIgnore;
import com.fasterxml.jackson.annotation.JsonProperty;
import com.shieldblaze.expressgateway.common.utils.NumberUtil;
import com.shieldblaze.expressgateway.configuration.Configuration;
import lombok.ToString;

/**
 * Configuration for HTTP/3 application-layer settings (RFC 9114).
 *
 * <p>QUIC transport parameters (idle timeout, flow control, stream limits, etc.)
 * have been moved to {@link com.shieldblaze.expressgateway.configuration.quic.QuicConfiguration}.
 * This configuration retains only HTTP/3-specific settings: QPACK compression
 * and Alt-Svc advertisement.</p>
 */
@ToString(exclude = "validated")
public final class Http3Configuration implements Configuration<Http3Configuration> {

    /**
     * Alt-Svc max-age in seconds for HTTP/2 to HTTP/3 migration.
     * Advertised via Alt-Svc header on HTTP/1.1 and HTTP/2 responses.
     * Set to 0 to disable Alt-Svc advertisement.
     */
    @JsonProperty
    private long altSvcMaxAge;

    /**
     * QUIC QPACK max table capacity in bytes (RFC 9204 Section 3.2.1).
     * Controls the maximum size of the QPACK dynamic table. Larger values
     * improve compression at the cost of memory per connection.
     */
    @JsonProperty
    private long qpackMaxTableCapacity;

    /**
     * QUIC QPACK blocked streams limit (RFC 9204 Section 3.2.2).
     * Maximum number of streams that can be blocked waiting for QPACK updates.
     */
    @JsonProperty
    private long qpackBlockedStreams;

    @JsonIgnore
    private boolean validated;

    public static final Http3Configuration DEFAULT = new Http3Configuration();

    static {
        DEFAULT.altSvcMaxAge = 0;
        DEFAULT.qpackMaxTableCapacity = 0;
        DEFAULT.qpackBlockedStreams = 0;
        DEFAULT.validated = true;
    }

    Http3Configuration() {
        // Prevent outside initialization
    }

    public long altSvcMaxAge() {
        assertValidated();
        return altSvcMaxAge;
    }

    public Http3Configuration setAltSvcMaxAge(long altSvcMaxAge) {
        this.altSvcMaxAge = altSvcMaxAge;
        return this;
    }

    public long qpackMaxTableCapacity() {
        assertValidated();
        return qpackMaxTableCapacity;
    }

    public Http3Configuration setQpackMaxTableCapacity(long qpackMaxTableCapacity) {
        this.qpackMaxTableCapacity = qpackMaxTableCapacity;
        return this;
    }

    public long qpackBlockedStreams() {
        assertValidated();
        return qpackBlockedStreams;
    }

    public Http3Configuration setQpackBlockedStreams(long qpackBlockedStreams) {
        this.qpackBlockedStreams = qpackBlockedStreams;
        return this;
    }

    @Override
    public Http3Configuration validate() {
        NumberUtil.checkZeroOrPositive(altSvcMaxAge, "AltSvcMaxAge");
        NumberUtil.checkZeroOrPositive(qpackMaxTableCapacity, "QpackMaxTableCapacity");
        NumberUtil.checkZeroOrPositive(qpackBlockedStreams, "QpackBlockedStreams");
        validated = true;
        return this;
    }

    @Override
    public boolean validated() {
        return validated;
    }
}
