/*
 * This file is part of ShieldBlaze ExpressGateway. [www.shieldblaze.com]
 * Copyright (c) 2020-2021 ShieldBlaze
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
package com.shieldblaze.expressgateway.configuration.tls;

import com.shieldblaze.expressgateway.common.utils.Number;
import io.netty.handler.ssl.SslContext;

import javax.net.ssl.SSLException;
import java.util.List;

/**
 * Configuration Builder for {@link TLSConfiguration}
 */
public final class TLSConfigurationBuilder {
    private final boolean forServer;

    private List<Cipher> ciphers;
    private List<Protocol> protocols;
    private MutualTLS mutualTLS = MutualTLS.NOT_REQUIRED;
    private boolean useStartTLS;
    private boolean useALPN;
    private int sessionTimeout;
    private int sessionCacheSize;
    private boolean acceptAllCerts;

    /**
     * @see TLSConfigurationBuilder#newBuilder(boolean)
     */
    private TLSConfigurationBuilder(boolean forServer) {
        this.forServer = forServer;
    }

    /**
     * Create a new {@link TLSConfigurationBuilder} Instance for TLS Client
     */
    public static TLSConfigurationBuilder forClient() {
        return new TLSConfigurationBuilder(false);
    }

    /**
     * Create a new {@link TLSConfigurationBuilder} Instance for TLS Server
     */
    public static TLSConfigurationBuilder forServer() {
        return new TLSConfigurationBuilder(true);
    }

    /**
     * Create a new {@link TLSConfigurationBuilder} Instance
     *
     * @param forServer {@code true} if building configuration for TLS Server else
     *                  set to {@code false} if building configuration for TLS Client.
     */
    public static TLSConfigurationBuilder newBuilder(boolean forServer) {
        return new TLSConfigurationBuilder(forServer);
    }

    /**
     * {@link List} of {@link Cipher}s to use
     */
    public TLSConfigurationBuilder withCiphers(List<Cipher> ciphers) {
        this.ciphers = ciphers;
        return this;
    }

    /**
     * {@link List} of {@link Protocol}s to use
     */
    public TLSConfigurationBuilder withProtocols(List<Protocol> protocols) {
        this.protocols = protocols;
        return this;
    }

    /**
     * {@link MutualTLS} to use for TLS Server
     */
    public TLSConfigurationBuilder withMutualTLS(MutualTLS mutualTLS) {
        this.mutualTLS = mutualTLS;
        return this;
    }

    /**
     * Set to {@code true} if we want to use {@code StartTLS} else set to {@code false}
     */
    public TLSConfigurationBuilder withUseStartTLS(boolean useStartTLS) {
        this.useStartTLS = useStartTLS;
        return this;
    }

    /**
     * Set to {@code true} if we want to use {@code ALPN} with HTTP/2 and HTTP/1.1 else set to {@code false}
     */
    public TLSConfigurationBuilder withUseALPN(boolean useALPN) {
        this.useALPN = useALPN;
        return this;
    }

    /**
     * Set Session Timeout for TLS Server
     */
    public TLSConfigurationBuilder withSessionTimeout(int sessionTimeout) {
        this.sessionTimeout = sessionTimeout;
        return this;
    }

    /**
     * Set Session Cache Size for TLS Server
     */
    public TLSConfigurationBuilder withSessionCacheSize(int sessionCacheSize) {
        this.sessionCacheSize = sessionCacheSize;
        return this;
    }

    public TLSConfigurationBuilder withAcceptAllCertificate(boolean acceptAllCerts) {
        this.acceptAllCerts = acceptAllCerts;
        return this;
    }

    /**
     * Build {@link TLSConfiguration}
     *
     * @return {@link TLSConfiguration} Instance
     * @throws SSLException             If there is an error while building {@link SslContext}
     * @throws NullPointerException     If a required value if {@code null}
     * @throws IllegalArgumentException If a required value is invalid
     */
    public TLSConfiguration build() throws SSLException {
        return new TLSConfiguration()
                .forServer(forServer)
                .acceptAllCerts(acceptAllCerts)
                .sessionCacheSize(Number.checkZeroOrPositive(sessionCacheSize, "Session Cache Size"))
                .sessionTimeout(Number.checkZeroOrPositive(sessionTimeout, "Session Timeout"))
                .protocols(protocols)
                .useStartTLS(useStartTLS)
                .mutualTLS(mutualTLS)
                .ciphers(ciphers)
                .useALPN(useALPN);
    }
}
