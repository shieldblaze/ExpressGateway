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

import com.google.gson.annotations.Expose;

import java.util.List;
import java.util.Map;
import java.util.concurrent.ConcurrentHashMap;

/**
 * Configuration for TLS
 */
public final class TLSConfiguration {

    @Expose
    private final Map<String, CertificateKeyPair> certificateKeyPairMap = new ConcurrentHashMap<>();

    @Expose
    private List<Cipher> ciphers;

    @Expose
    private List<Protocol> protocols;

    @Expose
    private MutualTLS mutualTLS;

    @Expose
    private boolean useStartTLS;

    @Expose
    private int sessionTimeout;

    @Expose
    private int sessionCacheSize;

    /**
     * Get {@link CertificateKeyPair} for a Hostname
     *
     * @param fqdn FQDN
     * @return {@link CertificateKeyPair} if found
     * @throws NullPointerException If Mapping is not found for a Hostname
     */
    public CertificateKeyPair mapping(String fqdn) {
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

    TLSConfiguration certificateKeyPairMap(Map<String, CertificateKeyPair> certificateKeyPairMap) {
        this.certificateKeyPairMap.putAll(certificateKeyPairMap);
        return this;
    }

    TLSConfiguration ciphers(List<Cipher> ciphers) {
        this.ciphers = ciphers;
        return this;
    }

    TLSConfiguration protocols(List<Protocol> protocols) {
        this.protocols = protocols;
        return this;
    }

    TLSConfiguration mutualTLS(MutualTLS mutualTLS) {
        this.mutualTLS = mutualTLS;
        return this;
    }

    TLSConfiguration useStartTLS(boolean useStartTLS) {
        this.useStartTLS = useStartTLS;
        return this;
    }

    TLSConfiguration sessionTimeout(int sessionTimeout) {
        this.sessionTimeout = sessionTimeout;
        return this;
    }

    TLSConfiguration sessionCacheSize(int sessionCacheSize) {
        this.sessionCacheSize = sessionCacheSize;
        return this;
    }

    public List<Cipher> ciphers() {
        return ciphers;
    }

    public List<Protocol> protocols() {
        return protocols;
    }

    public MutualTLS mutualTLS() {
        return mutualTLS;
    }

    public boolean useStartTLS() {
        return useStartTLS;
    }

    public int sessionTimeout() {
        return sessionTimeout;
    }

    public int sessionCacheSize() {
        return sessionCacheSize;
    }
}
