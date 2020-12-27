/*
 * This file is part of ShieldBlaze ExpressGateway. [www.shieldblaze.com]
 * Copyright (c) 2020 ShieldBlaze
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

import io.netty.handler.ssl.ApplicationProtocolConfig;
import io.netty.handler.ssl.ApplicationProtocolNames;
import io.netty.handler.ssl.OpenSsl;
import io.netty.handler.ssl.SslContext;
import io.netty.handler.ssl.SslContextBuilder;
import io.netty.handler.ssl.SslProvider;
import io.netty.handler.ssl.util.InsecureTrustManagerFactory;
import io.netty.util.internal.ObjectUtil;

import javax.net.ssl.SSLException;
import javax.net.ssl.TrustManager;
import javax.net.ssl.TrustManagerFactory;
import java.io.File;
import java.security.KeyStore;
import java.util.List;
import java.util.Map;
import java.util.TreeMap;

/**
 * Configuration Builder for {@link TLSConfiguration}
 */
public final class TLSConfigurationBuilder {
    private final boolean forServer;

    private List<Cipher> ciphers;
    private List<Protocol> protocols;
    private MutualTLS mutualTLS;
    private boolean useStartTLS;
    private int sessionTimeout;
    private int sessionCacheSize;
    private TLSServerMapping tlsServerMapping;
    private CertificateKeyPair certificateKeyPair;
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

    /**
     * Add {@link TLSServerMapping} for TLS Server
     */
    public TLSConfigurationBuilder withTLSServerMapping(TLSServerMapping tlsServerMapping) {
        this.tlsServerMapping = tlsServerMapping;
        return this;
    }

    /**
     * Add {@link CertificateKeyPair} for TLS Client - Mutual TLS
     */
    public TLSConfigurationBuilder withClientCertificateKeyPair(CertificateKeyPair certificateKeyPair) {
        this.certificateKeyPair = certificateKeyPair;
        return this;
    }

    /**
     * Accept all certificates (Insecure)
     */
    public TLSConfigurationBuilder withAcceptAllCerts(boolean acceptAllCerts) {
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

        ObjectUtil.checkNonEmpty(ciphers, "Ciphers");
        ObjectUtil.checkNonEmpty(protocols, "Protocols");
        ObjectUtil.checkNotNull(mutualTLS, "MutualTLS");

        // For Server, we need TLS Server Mapping
        // For Client, We need Trust Manager and Mutual TLS
        if (forServer) {
            ObjectUtil.checkNotNull(tlsServerMapping, "TLSServerMapping");
        } else {
            if (mutualTLS == MutualTLS.REQUIRED || mutualTLS == MutualTLS.OPTIONAL) {
                ObjectUtil.checkNotNull(certificateKeyPair, "CertificateKeyPair for Client");
            }
        }

        Map<String, CertificateKeyPair> hostnameMap = new TreeMap<>();

        if (forServer) {

            for (Map.Entry<String, CertificateKeyPair> entry : tlsServerMapping.certificateKeyMap.entrySet()) {
                String hostname = entry.getKey();
                CertificateKeyPair certificateKeyPair = entry.getValue();

                // Throw error if OpenSsl does not support OCSP Stapling
                if (certificateKeyPair.useOCSPStapling() && !OpenSsl.isOcspSupported()) {
                    throw new IllegalArgumentException("OCSP Stapling is not available because OpenSSL is not available");
                }

                SslContextBuilder sslContextBuilder = SslContextBuilder.forServer(new File(certificateKeyPair.certificateChain()),
                        new File(certificateKeyPair.privateKey()))
                        .sslProvider(OpenSsl.isAvailable() ? SslProvider.OPENSSL : SslProvider.JDK)
                        .protocols(Protocol.getProtocols(protocols))
                        .enableOcsp(certificateKeyPair.useOCSPStapling())
                        .clientAuth(mutualTLS.clientAuth())
                        .startTls(useStartTLS)
                        .sessionTimeout(ObjectUtil.checkPositiveOrZero(sessionTimeout, "Session Timeout"))
                        .sessionCacheSize(ObjectUtil.checkPositiveOrZero(sessionCacheSize, "Session Cache Size"))
                        .applicationProtocolConfig(new ApplicationProtocolConfig(
                                ApplicationProtocolConfig.Protocol.ALPN,
                                ApplicationProtocolConfig.SelectorFailureBehavior.NO_ADVERTISE,
                                ApplicationProtocolConfig.SelectedListenerFailureBehavior.ACCEPT,
                                ApplicationProtocolNames.HTTP_2,
                                ApplicationProtocolNames.HTTP_1_1));

                certificateKeyPair.setSslContext(sslContextBuilder.build());
                hostnameMap.put(hostname, certificateKeyPair);
            }
        } else {
            TrustManagerFactory trustManagerFactory;
            if (acceptAllCerts) {
                trustManagerFactory = InsecureTrustManagerFactory.INSTANCE;
            } else {
                try {
                    trustManagerFactory = TrustManagerFactory.getInstance(TrustManagerFactory.getDefaultAlgorithm());
                    trustManagerFactory.init((KeyStore) null);
                } catch (Exception ex) {
                    throw new IllegalArgumentException("Error occurred while building TrustManagerFactory", ex);
                }
            }

            SslContextBuilder sslContextBuilder = SslContextBuilder.forClient()
                    .sslProvider(OpenSsl.isAvailable() ? SslProvider.OPENSSL : SslProvider.JDK)
                    .protocols(Protocol.getProtocols(protocols))
                    .clientAuth(mutualTLS.clientAuth())
                    .trustManager(trustManagerFactory)
                    .startTls(useStartTLS)
                    .applicationProtocolConfig(new ApplicationProtocolConfig(
                            ApplicationProtocolConfig.Protocol.ALPN,
                            ApplicationProtocolConfig.SelectorFailureBehavior.NO_ADVERTISE,
                            ApplicationProtocolConfig.SelectedListenerFailureBehavior.ACCEPT,
                            ApplicationProtocolNames.HTTP_2,
                            ApplicationProtocolNames.HTTP_1_1));

            if (certificateKeyPair != null) {
                if (mutualTLS == MutualTLS.REQUIRED || mutualTLS == MutualTLS.OPTIONAL) {
                    sslContextBuilder.keyManager(new File(certificateKeyPair.certificateChain()).getAbsoluteFile(),
                            new File(certificateKeyPair.privateKey()).getAbsoluteFile());
                }
            }

            if (certificateKeyPair == null) {
                certificateKeyPair = new CertificateKeyPair();
            }

            certificateKeyPair.setSslContext(sslContextBuilder.build());
            hostnameMap.put("DEFAULT_HOST", certificateKeyPair);
        }

        return new TLSConfiguration()
                .certificateKeyPairMap(hostnameMap)
                .ciphers(ciphers)
                .protocols(protocols)
                .mutualTLS(mutualTLS)
                .useStartTLS(useStartTLS)
                .sessionTimeout(sessionTimeout)
                .sessionCacheSize(sessionCacheSize)
                .acceptAllCerts(acceptAllCerts);
    }
}
