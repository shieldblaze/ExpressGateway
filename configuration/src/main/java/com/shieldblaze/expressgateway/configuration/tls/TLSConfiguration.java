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
package com.shieldblaze.expressgateway.configuration.tls;

import com.fasterxml.jackson.annotation.JsonIgnore;
import com.fasterxml.jackson.annotation.JsonProperty;
import com.shieldblaze.expressgateway.common.utils.ListUtil;
import com.shieldblaze.expressgateway.common.utils.NumberUtil;
import com.shieldblaze.expressgateway.configuration.Configuration;

import javax.net.ssl.SSLException;
import java.util.HashMap;
import java.util.List;
import java.util.Map;
import java.util.Objects;
import java.util.concurrent.ConcurrentHashMap;

/**
 * Configuration for TLS
 */
public abstract class TLSConfiguration implements Configuration {

    @JsonIgnore
    private final Map<String, CertificateKeyPair> certificateKeyPairMap = new HashMap<>();

    @JsonProperty("enabled")
    private boolean enabled = false;

    @JsonProperty("ciphers")
    protected List<Cipher> ciphers;

    @JsonProperty("protocols")
    protected List<Protocol> protocols;

    @JsonProperty("mutualTLS")
    protected MutualTLS mutualTLS = MutualTLS.NOT_REQUIRED;

    @JsonProperty("useStartTLS")
    protected boolean useStartTLS;

    @JsonProperty("sessionTimeout")
    protected int sessionTimeout;

    @JsonProperty("sessionCacheSize")
    protected int sessionCacheSize;

    @JsonProperty("acceptAllCerts")
    protected boolean acceptAllCerts;

    /**
     * Add the default {@link CertificateKeyPair} mapping
     */
    public void defaultMapping(CertificateKeyPair certificateKeyPair) throws SSLException {
        addMapping("DEFAULT", certificateKeyPair);
    }

    /**
     * Get default mapping {@link CertificateKeyPair}
     */
    public CertificateKeyPair defaultMapping() {
        return certificateKeyPairMap.get("DEFAULT");
    }

    /**
     * Add a new mapping
     *
     * @param host               FQDN
     * @param certificateKeyPair {@link CertificateKeyPair} Instance
     */
    public void addMapping(String host, CertificateKeyPair certificateKeyPair) throws SSLException {
        certificateKeyPairMap.put(host, certificateKeyPair);
        certificateKeyPair.init(this);
    }

    /**
     * Remove mapping for Host
     *
     * @param host Host to be removed
     * @return {@code true} if mapping is successfully removed else {@code false}
     */
    public boolean removeMapping(String host) {
        return certificateKeyPairMap.remove(host) != null;
    }

    /**
     * Remove all mappings
     */
    public void clearMappings() {
        certificateKeyPairMap.clear();
    }

    /**
     * Get {@link CertificateKeyPair} for a Hostname
     *
     * @param fqdn FQDN
     * @return {@link CertificateKeyPair} if found
     * @throws NullPointerException If Mapping is not found for a Hostname
     */
    public CertificateKeyPair mapping(String fqdn) {
        String _fqdn = fqdn;
        try {
            CertificateKeyPair certificateKeyPair = certificateKeyPairMap.get(fqdn);

            // If `null` then it means mapping was not found with FQDN.
            // We'll try wildcard now.
            if (certificateKeyPair == null) {
                fqdn = "*" + fqdn.substring(fqdn.indexOf("."));
                certificateKeyPair = certificateKeyPairMap.get(fqdn);
                if (certificateKeyPair != null) {
                    return certificateKeyPair;
                }
            } else {
                return certificateKeyPair;
            }
        } catch (Exception ex) {
            // Ignore
        }

        throw new NullPointerException("Mapping not found for Hostname: " + _fqdn);
    }

    public TLSConfiguration enable() {
        enabled = true;
        return this;
    }

    protected TLSConfiguration disable() {
        enabled = false;
        return this;
    }

    /**
     * Check whether this configuration is enabled or not
     *
     * @return {@code true} if enabled else {@code false}
     */
    public boolean enabled() {
        return enabled;
    }

    TLSConfiguration setCiphers(List<Cipher> ciphers) {
        ListUtil.checkNonEmpty(ciphers, "Ciphers");
        this.ciphers = ciphers;
        return this;
    }

    TLSConfiguration setProtocols(List<Protocol> protocols) {
        ListUtil.checkNonEmpty(protocols, "Protocols");
        this.protocols = protocols;
        return this;
    }

    TLSConfiguration setMutualTLS(MutualTLS mutualTLS) {
        Objects.requireNonNull(mutualTLS, "MutualTLS");
        this.mutualTLS = mutualTLS;
        return this;
    }

    TLSConfiguration setUseStartTLS(boolean useStartTLS) {
        this.useStartTLS = useStartTLS;
        return this;
    }

    TLSConfiguration setSessionTimeout(int sessionTimeout) {
        NumberUtil.checkZeroOrPositive(sessionTimeout, "Session Timeout");
        this.sessionTimeout = sessionTimeout;
        return this;
    }

    TLSConfiguration setSessionCacheSize(int sessionCacheSize) {
        NumberUtil.checkZeroOrPositive(sessionCacheSize, "Session Cache Size");
        this.sessionCacheSize = sessionCacheSize;
        return this;
    }

    TLSConfiguration setAcceptAllCerts(boolean acceptAllCerts) {
        this.acceptAllCerts = acceptAllCerts;
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

    public boolean acceptAllCerts() {
        return acceptAllCerts;
    }

    /**
     * Validate all parameters of this configuration
     *
     * @return this class instance
     * @throws IllegalArgumentException If any value is invalid
     * @throws NullPointerException If any value is null
     */
    public TLSConfiguration validate() throws IllegalArgumentException, NullPointerException {
        ListUtil.checkNonEmpty(ciphers, "Ciphers");
        ListUtil.checkNonEmpty(protocols, "Protocols");
        Objects.requireNonNull(mutualTLS, "MutualTLS");
        NumberUtil.checkZeroOrPositive(sessionTimeout, "Session Timeout");
        NumberUtil.checkZeroOrPositive(sessionCacheSize, "Session Cache Size");
        return this;
    }
}
