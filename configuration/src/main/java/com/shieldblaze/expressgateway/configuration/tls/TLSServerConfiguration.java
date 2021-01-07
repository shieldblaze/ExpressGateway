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

import com.fasterxml.jackson.annotation.JsonProperty;
import com.google.gson.annotations.Expose;
import com.shieldblaze.expressgateway.common.utils.Number;
import com.shieldblaze.expressgateway.configuration.Configuration;
import io.netty.handler.ssl.ApplicationProtocolConfig;
import io.netty.handler.ssl.ApplicationProtocolNames;
import io.netty.handler.ssl.OpenSsl;
import io.netty.handler.ssl.SslContext;
import io.netty.handler.ssl.SslContextBuilder;
import io.netty.handler.ssl.SslProvider;

import javax.net.ssl.SSLException;
import java.io.File;
import java.util.ArrayList;
import java.util.List;
import java.util.Map;
import java.util.Objects;
import java.util.concurrent.ConcurrentHashMap;

@SuppressWarnings("unused")
public final class TLSServerConfiguration implements Configuration {

    public static final TLSServerConfiguration EMPTY_INSTANCE = new TLSServerConfiguration();

    @Expose
    @JsonProperty("certificateKeyPairMap")
    private final Map<String, CertificateKeyPair> certificateKeyPairMap = new ConcurrentHashMap<>();

    @Expose
    @JsonProperty("ciphers")
    private List<Cipher> ciphers;

    @Expose
    @JsonProperty("protocols")
    private List<Protocol> protocols;

    @Expose
    @JsonProperty("mutualTLS")
    private MutualTLS mutualTLS;

    @Expose
    @JsonProperty("useStartTLS")
    private boolean useStartTLS;

    @Expose
    @JsonProperty("sessionTimeout")
    private int sessionTimeout;

    @Expose
    @JsonProperty("sessionCacheSize")
    private int sessionCacheSize;

    /**
     * Get {@link CertificateKeyPair} for a Hostname
     *
     * @param fqdn FQDN
     * @return {@link CertificateKeyPair} if found
     * @throws NullPointerException If Mapping is not found for a Hostname
     */
    public CertificateKeyPair getMapping(String fqdn) {
        try {
            CertificateKeyPair certificateKeyPair = certificateKeyPairMap.get(fqdn);

            // If `null` it means, Mapping was not found with FQDN then we'll try Wildcard.
            if (certificateKeyPair == null) {
                fqdn = "*" + fqdn.substring(fqdn.indexOf("."));
                certificateKeyPair = certificateKeyPairMap.get(fqdn);
                if (certificateKeyPair != null) {
                    return certificateKeyPair;
                }
            }
        } catch (Exception ex) {
            // Ignore
        }

        if (certificateKeyPairMap.containsKey("DEFAULT_HOST")) {
            return defaultMapping();
        }

        throw new NullPointerException("Mapping Not Found");
    }

    public CertificateKeyPair defaultMapping() {
        return certificateKeyPairMap.get("DEFAULT_HOST");
    }

    public TLSServerConfiguration defaultMapping(CertificateKeyPair certificateKeyPair) throws SSLException {
        return addMapping("DEFAULT_HOST", certificateKeyPair);
    }

    public TLSServerConfiguration addMapping(String hostname, CertificateKeyPair certificateKeyPair) throws SSLException {
        // Throw error if OpenSsl does not support OCSP Stapling
        if (certificateKeyPair.useOCSPStapling() && !OpenSsl.isOcspSupported()) {
            throw new IllegalArgumentException("OCSP Stapling is not available because OpenSSL is not available");
        }

        List<String> _ciphers = new ArrayList<>();
        for (Cipher cipher : ciphers) {
            _ciphers.add(cipher.toString());
        }

        SslContext sslContext = SslContextBuilder.forServer(new File(certificateKeyPair.certificateChain()), new File(certificateKeyPair.privateKey()))
                .sslProvider(OpenSsl.isAvailable() ? SslProvider.OPENSSL : SslProvider.JDK)
                .protocols(Protocol.getProtocols(protocols))
                .ciphers(_ciphers)
                .enableOcsp(certificateKeyPair.useOCSPStapling())
                .clientAuth(mutualTLS.clientAuth())
                .startTls(useStartTLS)
                .sessionTimeout(sessionTimeout)
                .sessionCacheSize(sessionCacheSize)
                .applicationProtocolConfig(new ApplicationProtocolConfig(
                        ApplicationProtocolConfig.Protocol.ALPN,
                        ApplicationProtocolConfig.SelectorFailureBehavior.NO_ADVERTISE,
                        ApplicationProtocolConfig.SelectedListenerFailureBehavior.ACCEPT,
                        ApplicationProtocolNames.HTTP_2,
                        ApplicationProtocolNames.HTTP_1_1))
                .build();

        certificateKeyPair.setSslContext(sslContext);
        certificateKeyPairMap.put(hostname, certificateKeyPair);
        return this;
    }

    public TLSServerConfiguration ciphers(List<Cipher> ciphers) {
        this.ciphers = ciphers;
        return this;
    }

    public TLSServerConfiguration protocols(List<Protocol> protocols) {
        this.protocols = protocols;
        return this;
    }

    public TLSServerConfiguration mutualTLS(MutualTLS mutualTLS) {
        this.mutualTLS = mutualTLS;
        return this;
    }

    public TLSServerConfiguration useStartTLS(boolean useStartTLS) {
        this.useStartTLS = useStartTLS;
        return this;
    }

    public TLSServerConfiguration sessionTimeout(int sessionTimeout) {
        this.sessionTimeout = sessionTimeout;
        return this;
    }

    public TLSServerConfiguration sessionCacheSize(int sessionCacheSize) {
        this.sessionCacheSize = sessionCacheSize;
        return this;
    }

    @Override
    public String name() {
        return "TLSServer";
    }

    @Override
    public void validate() throws Exception {
        Objects.requireNonNull(ciphers);
        Objects.requireNonNull(protocols);
        Objects.requireNonNull(mutualTLS);
        Number.checkPositive(sessionTimeout, "Session Timeout");
        Number.checkPositive(sessionCacheSize, "Session Cache Size");
    }
}
