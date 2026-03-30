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
package com.shieldblaze.expressgateway.protocol.http.http3.loadbalancer;

import com.shieldblaze.expressgateway.configuration.ConfigurationContext;
import com.shieldblaze.expressgateway.configuration.tls.CertificateKeyPair;
import com.shieldblaze.expressgateway.configuration.tls.TlsServerConfiguration;
import com.shieldblaze.expressgateway.core.L4FrontListener;
import com.shieldblaze.expressgateway.protocol.http.http3.Http3Listener;
import io.netty.handler.codec.http3.Http3;
import io.netty.handler.codec.quic.QuicSslContext;
import io.netty.handler.codec.quic.QuicSslContextBuilder;

import java.net.InetSocketAddress;
import java.util.Objects;

/**
 * Builder for {@link Http3LoadBalancer}.
 *
 * <p>Creates an HTTP/3 load balancer backed by QUIC transport. The builder
 * defaults to {@link Http3Listener} for the frontend listener, which binds
 * a UDP socket and runs QUIC server-side handshake and HTTP/3 framing.</p>
 *
 * <p>QUIC must be explicitly enabled in
 * {@link com.shieldblaze.expressgateway.configuration.quic.QuicConfiguration}
 * before the load balancer will accept QUIC connections.</p>
 */
public final class Http3LoadBalancerBuilder {

    private String name;
    private InetSocketAddress bindAddress;
    private ConfigurationContext configurationContext = ConfigurationContext.DEFAULT;
    private L4FrontListener l4FrontListener;
    private QuicSslContext quicSslContext;

    private Http3LoadBalancerBuilder() {
        // Prevent outside initialization
    }

    public static Http3LoadBalancerBuilder newBuilder() {
        return new Http3LoadBalancerBuilder();
    }

    public Http3LoadBalancerBuilder withName(String name) {
        this.name = name;
        return this;
    }

    public Http3LoadBalancerBuilder withBindAddress(InetSocketAddress bindAddress) {
        this.bindAddress = bindAddress;
        return this;
    }

    public Http3LoadBalancerBuilder withConfigurationContext(ConfigurationContext configurationContext) {
        this.configurationContext = configurationContext;
        return this;
    }

    public Http3LoadBalancerBuilder withL4FrontListener(L4FrontListener l4FrontListener) {
        this.l4FrontListener = l4FrontListener;
        return this;
    }

    public Http3LoadBalancerBuilder withQuicSslContext(QuicSslContext quicSslContext) {
        this.quicSslContext = quicSslContext;
        return this;
    }

    /**
     * Build the {@link Http3LoadBalancer}.
     *
     * @throws NullPointerException     if bindAddress is null
     * @throws IllegalStateException    if QUIC is not enabled in the configuration
     */
    public Http3LoadBalancer build() {
        Objects.requireNonNull(bindAddress, "BindAddress");

        if (!configurationContext.quicConfiguration().enabled()) {
            throw new IllegalStateException(
                    "QUIC is not enabled in QuicConfiguration. Set enabled=true before building.");
        }

        if (l4FrontListener == null) {
            if (quicSslContext == null) {
                TlsServerConfiguration tlsConfig = configurationContext.tlsServerConfiguration();
                CertificateKeyPair certKeyPair = tlsConfig.defaultMapping();
                if (certKeyPair == null) {
                    throw new IllegalStateException(
                            "No default TLS certificate mapping configured. " +
                            "QUIC requires TLS 1.3 (RFC 9001) — provide a QuicSslContext or configure a default certificate.");
                }
                try {
                    java.security.PrivateKey privateKey = certKeyPair.privateKey();
                    java.util.List<java.security.cert.X509Certificate> certs = certKeyPair.certificates();
                    QuicSslContextBuilder sslBuilder = QuicSslContextBuilder.forServer(
                                    privateKey, null,
                                    certs.toArray(new java.security.cert.X509Certificate[0]))
                            .applicationProtocols(Http3.supportedApplicationProtocols());

                    // Wire 0-RTT (early data) from QuicConfiguration. Without this, the
                    // zeroRttEnabled flag in QuicConfiguration had no effect on the server-side
                    // TLS context, making 0-RTT handshakes impossible even when configured.
                    // The client-side wiring in QuicBootstrapper already calls earlyData(true),
                    // but the server must also enable it for 0-RTT to work bidirectionally.
                    if (configurationContext.quicConfiguration().zeroRttEnabled()) {
                        sslBuilder.earlyData(true);
                    }

                    quicSslContext = sslBuilder.build();
                } catch (Exception e) {
                    throw new IllegalStateException("Failed to build QUIC TLS context from TLS server configuration", e);
                }
            }
            l4FrontListener = new Http3Listener(quicSslContext);
        }

        return new Http3LoadBalancer(
                name,
                bindAddress,
                l4FrontListener,
                configurationContext
        );
    }
}
