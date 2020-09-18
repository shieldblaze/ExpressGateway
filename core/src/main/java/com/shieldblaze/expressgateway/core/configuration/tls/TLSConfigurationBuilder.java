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
package com.shieldblaze.expressgateway.core.configuration.tls;

import io.netty.handler.ssl.ApplicationProtocolConfig;
import io.netty.handler.ssl.ApplicationProtocolNames;
import io.netty.handler.ssl.OpenSsl;
import io.netty.handler.ssl.SslContext;
import io.netty.handler.ssl.SslContextBuilder;
import io.netty.handler.ssl.SslProvider;
import io.netty.util.internal.ObjectUtil;

import javax.net.ssl.SSLException;
import javax.net.ssl.TrustManager;
import java.security.PrivateKey;
import java.security.cert.X509Certificate;
import java.util.List;
import java.util.Map;
import java.util.Optional;
import java.util.TreeMap;

public final class TLSConfigurationBuilder {
    private final boolean forServer;

    private List<Ciphers> ciphers;
    private List<Protocol> protocols;
    private PrivateKey privateKey;
    private List<X509Certificate> certificateChain;
    private TrustManager trustManager;
    private MutualTLS mutualTLS;
    private boolean useStartTLS;
    private boolean enableOCSPStapling;
    private boolean enableOCSPCheck;
    private boolean useALPN;
    private int sessionTimeout;
    private int sessionCacheSize;
    private TLSServerMapping tlsServerMapping;

    private TLSConfigurationBuilder(boolean forServer) {
        this.forServer = forServer;
    }

    public static TLSConfigurationBuilder forClient() {
        return new TLSConfigurationBuilder(false);
    }

    public static TLSConfigurationBuilder forServer() {
        return new TLSConfigurationBuilder(true);
    }

    public static TLSConfigurationBuilder newBuilder(boolean forServer) {
        return new TLSConfigurationBuilder(forServer);
    }

    public TLSConfigurationBuilder withCiphers(List<Ciphers> ciphers) {
        this.ciphers = ciphers;
        return this;
    }

    public TLSConfigurationBuilder withProtocols(List<Protocol> protocols) {
        this.protocols = protocols;
        return this;
    }

    public TLSConfigurationBuilder withPrivateKey(PrivateKey privateKey) {
        this.privateKey = privateKey;
        return this;
    }

    public TLSConfigurationBuilder withCertificateChain(List<X509Certificate> certificateChain) {
        this.certificateChain = certificateChain;
        return this;
    }

    public TLSConfigurationBuilder withTrustManager(TrustManager trustManager) {
        this.trustManager = trustManager;
        return this;
    }

    public TLSConfigurationBuilder withMutualTLS(MutualTLS mutualTLS) {
        this.mutualTLS = mutualTLS;
        return this;
    }

    public TLSConfigurationBuilder withUseStartTLS(boolean useStartTLS) {
        this.useStartTLS = useStartTLS;
        return this;
    }

    public TLSConfigurationBuilder withEnableOCSPStapling(boolean enableOCSPStapling) {
        this.enableOCSPStapling = enableOCSPStapling;
        return this;
    }

    public TLSConfigurationBuilder withEnableOCSPCheck(boolean enableOCSPCheck) {
        this.enableOCSPCheck = enableOCSPCheck;
        return this;
    }

    public TLSConfigurationBuilder withUseALPN(boolean useALPN) {
        this.useALPN = useALPN;
        return this;
    }

    public TLSConfigurationBuilder withSessionTimeout(int sessionTimeout) {
        this.sessionTimeout = sessionTimeout;
        return this;
    }

    public TLSConfigurationBuilder withSessionCacheSize(int sessionCacheSize) {
        this.sessionCacheSize = sessionCacheSize;
        return this;
    }

    public TLSConfigurationBuilder withTLSServerMapping(TLSServerMapping tlsServerMapping) {
        this.tlsServerMapping = tlsServerMapping;
        return this;
    }

    public TLSConfiguration build() throws SSLException {

        // Throw error if OpenSsl does not support OCSP Stapling
        if (forServer && enableOCSPStapling && !OpenSsl.isOcspSupported()) {
            throw new IllegalArgumentException("OCSP Stapling is not available because OpenSsl is not loaded.");
        }

        ObjectUtil.checkNonEmpty(ciphers, "Ciphers");
        ObjectUtil.checkNonEmpty(protocols, "Protocols");

        if (!forServer) {
            ObjectUtil.checkNotNull(trustManager, "Trust Manager");
            ObjectUtil.checkNotNull(mutualTLS, "MutualTLS");

            if (mutualTLS == MutualTLS.REQUIRED || mutualTLS == MutualTLS.OPTIONAL) {
                ObjectUtil.checkNotNull(privateKey, "Private Key");
                ObjectUtil.checkNonEmpty(certificateChain, "Certificate Chain");
            }
        }

        if (forServer) {
            ObjectUtil.checkNotNull(tlsServerMapping, "TLS Mapping");

            if (tlsServerMapping.hasDefaultMapping) {
                Optional<String> optional = tlsServerMapping.certificateKeyMap.keySet()
                        .stream()
                        .filter(hostname -> hostname.equalsIgnoreCase("DEFAULT_HOST"))
                        .findAny();

                if (!optional.isPresent()) {
                    throw new IllegalArgumentException("Default Host Not Present");
                }
            }
        }

        Map<String, SslContext> hostnameMap = new TreeMap<>();

        if (forServer) {

            for (Map.Entry<String, ServerCertificateKey> entry : tlsServerMapping.certificateKeyMap.entrySet()) {
                String hostname = entry.getKey();
                ServerCertificateKey serverCertificateKey = entry.getValue();

                SslContextBuilder sslContextBuilder =
                        SslContextBuilder.forServer(serverCertificateKey.getPrivateKey(), serverCertificateKey.getCertificateChain())
                                .sslProvider(OpenSsl.isAvailable() ? SslProvider.OPENSSL : SslProvider.JDK)
                                .protocols(Protocol.getProtocols(protocols))
                                .enableOcsp(enableOCSPStapling)
                                .clientAuth(mutualTLS.getClientAuth())
                                .startTls(useStartTLS)
                                .sessionTimeout(ObjectUtil.checkPositiveOrZero(sessionTimeout, "Session Timeout"))
                                .sessionCacheSize(ObjectUtil.checkPositiveOrZero(sessionCacheSize, "Session Cache Size"));


                if (useALPN) {
                    sslContextBuilder.applicationProtocolConfig(new ApplicationProtocolConfig(
                            ApplicationProtocolConfig.Protocol.ALPN,
                            ApplicationProtocolConfig.SelectorFailureBehavior.NO_ADVERTISE,
                            ApplicationProtocolConfig.SelectedListenerFailureBehavior.ACCEPT,
                            ApplicationProtocolNames.HTTP_2,
                            ApplicationProtocolNames.HTTP_1_1));
                }

                hostnameMap.put(hostname, sslContextBuilder.build());
            }
        } else {
            SslContextBuilder sslContextBuilder = SslContextBuilder.forClient()
                    .sslProvider(OpenSsl.isAvailable() ? SslProvider.OPENSSL : SslProvider.JDK)
                    .protocols(Protocol.getProtocols(protocols))
                    .enableOcsp(enableOCSPStapling)
                    .clientAuth(mutualTLS.getClientAuth())
                    .startTls(useStartTLS);

            if (mutualTLS == MutualTLS.REQUIRED || mutualTLS == MutualTLS.OPTIONAL) {
                sslContextBuilder.keyManager(privateKey, certificateChain);
            }

            hostnameMap.put("DEFAULT_HOST", sslContextBuilder.build());
        }

        TLSConfiguration tLSConfiguration = new TLSConfiguration();
        tLSConfiguration.enableOCSPCheck(enableOCSPCheck);
        tLSConfiguration.enableOCSPStapling(enableOCSPStapling);
        tLSConfiguration.setHostnameCertificateMapping(hostnameMap);
        System.out.println(hostnameMap);
        tLSConfiguration.setForServer(forServer);
        return tLSConfiguration;
    }
}
