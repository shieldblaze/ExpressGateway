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
import dev.morphia.annotations.Property;
import dev.morphia.annotations.Transient;

import javax.net.ssl.SSLException;
import java.util.HashMap;
import java.util.List;
import java.util.Map;
import java.util.Objects;

/**
 * Configuration for TLS
 */
public abstract class TlsConfiguration implements Configuration<TlsConfiguration> {

    @Transient
    @JsonIgnore
    private final Map<String, CertificateKeyPair> certificateKeyPairMap = new HashMap<>();

    @Property
    @JsonProperty("enabled")
    private boolean enabled = false;

    @Property
    @JsonProperty("ciphers")
    protected List<Cipher> ciphers;

    @Property
    @JsonProperty("protocols")
    protected List<Protocol> protocols;

    @Property
    @JsonProperty("mutualTLS")
    protected MutualTLS mutualTLS = MutualTLS.NOT_REQUIRED;

    @Property
    @JsonProperty("useStartTLS")
    protected boolean useStartTLS;

    @Property
    @JsonProperty("sessionTimeout")
    protected int sessionTimeout;

    @Property
    @JsonProperty("sessionCacheSize")
    protected int sessionCacheSize;

    @Property
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

    /**
     * Enable this TLS Configuration
     */
    public TlsConfiguration enable() {
        enabled = true;
        return this;
    }

    /**
     * Disable this TLS Configuration
     */
    public TlsConfiguration disable() {
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

    /**
     * {@link List} of {@link Cipher}s to use
     */
    public TlsConfiguration setCiphers(List<Cipher> ciphers) {
        ListUtil.checkNonEmpty(ciphers, "Ciphers");
        this.ciphers = ciphers;
        return this;
    }

    /**
     * {@link List} of {@link Cipher}s to use
     */
    public List<Cipher> ciphers() {
        return ciphers;
    }

    /**
     * {@link List} of {@link Protocol}s to use
     */
    public TlsConfiguration setProtocols(List<Protocol> protocols) {
        ListUtil.checkNonEmpty(protocols, "Protocols");
        this.protocols = protocols;
        return this;
    }

    /**
     * {@link List} of {@link Protocol}s to use
     */
    public List<Protocol> protocols() {
        return protocols;
    }

    /**
     * {@link MutualTLS} to use for TLS Server
     */
    public TlsConfiguration setMutualTLS(MutualTLS mutualTLS) {
        Objects.requireNonNull(mutualTLS, "MutualTLS");
        this.mutualTLS = mutualTLS;
        return this;
    }

    /**
     * {@link MutualTLS} to use for TLS Server
     */
    public MutualTLS mutualTLS() {
        return mutualTLS;
    }

    /**
     * Set to {@code true} if we want to use {@code StartTLS} else set to {@code false}
     */
    public TlsConfiguration setUseStartTLS(boolean useStartTLS) {
        this.useStartTLS = useStartTLS;
        return this;
    }

    /**
     * Set to {@code true} if we want to use {@code StartTLS} else set to {@code false}
     */
    public boolean useStartTLS() {
        return useStartTLS;
    }

    /**
     * Set Session Timeout for TLS Server
     */
    public TlsConfiguration setSessionTimeout(int sessionTimeout) {
        NumberUtil.checkZeroOrPositive(sessionTimeout, "Session Timeout");
        this.sessionTimeout = sessionTimeout;
        return this;
    }

    /**
     * Set Session Timeout for TLS Server
     */
    public int sessionTimeout() {
        return sessionTimeout;
    }

    /**
     * Set Session Cache Size for TLS Server
     */
    public TlsConfiguration setSessionCacheSize(int sessionCacheSize) {
        NumberUtil.checkZeroOrPositive(sessionCacheSize, "Session Cache Size");
        this.sessionCacheSize = sessionCacheSize;
        return this;
    }

    /**
     * Set Session Cache Size for TLS Server
     */
    public int sessionCacheSize() {
        return sessionCacheSize;
    }

    /**
     * Accept all certificates (including self-signed certificates)
     */
    public TlsConfiguration setAcceptAllCerts(boolean acceptAllCerts) {
        this.acceptAllCerts = acceptAllCerts;
        return this;
    }

    /**
     * Accept all certificates (including self-signed certificates)
     */
    public boolean acceptAllCerts() {
        return acceptAllCerts;
    }

    /**
     * Validate all parameters of this configuration
     *
     * @return this class instance
     * @throws IllegalArgumentException If any value is invalid
     * @throws NullPointerException     If any value is null
     */
    public TlsConfiguration validate() throws IllegalArgumentException, NullPointerException {
        ListUtil.checkNonEmpty(ciphers, "Ciphers");
        ListUtil.checkNonEmpty(protocols, "Protocols");
        Objects.requireNonNull(mutualTLS, "MutualTLS");
        NumberUtil.checkZeroOrPositive(sessionTimeout, "Session Timeout");
        NumberUtil.checkZeroOrPositive(sessionCacheSize, "Session Cache Size");
        enabled();
        return this;
    }
}
