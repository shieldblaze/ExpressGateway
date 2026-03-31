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
import com.shieldblaze.expressgateway.common.ExpressGateway;
import com.shieldblaze.expressgateway.common.utils.ListUtil;
import com.shieldblaze.expressgateway.common.utils.NumberUtil;
import com.shieldblaze.expressgateway.common.zookeeper.Environment;
import com.shieldblaze.expressgateway.configuration.Configuration;
import org.apache.logging.log4j.LogManager;
import org.apache.logging.log4j.Logger;

import javax.net.ssl.SSLException;
import java.util.List;
import java.util.Map;
import java.util.Objects;
import java.util.concurrent.ConcurrentHashMap;
import java.util.concurrent.locks.ReentrantReadWriteLock;

/**
 * Configuration for TLS
 */
public abstract class TlsConfiguration implements Configuration<TlsConfiguration> {

    private static final Logger logger = LogManager.getLogger(TlsConfiguration.class);

    @JsonIgnore
    protected final Map<String, CertificateKeyPair> certificateKeyPairMap = new ConcurrentHashMap<>();

    /**
     * Lock for hot certificate reloading. Readers acquire the read lock when using SslContext;
     * the reload method acquires the write lock to atomically swap contexts.
     */
    @JsonIgnore
    protected final ReentrantReadWriteLock reloadLock = new ReentrantReadWriteLock();

    @JsonProperty("enabled")
    private boolean enabled;

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
     * @param host               FQDN or wildcard (must start with {@code *.} for wildcard certs)
     * @param certificateKeyPair {@link CertificateKeyPair} Instance
     * @throws IllegalArgumentException if wildcard host does not start with {@code *.}
     */
    public void addMapping(String host, CertificateKeyPair certificateKeyPair) throws SSLException {
        // LOW-14: Validate that wildcard keys start with "*."
        if (host != null && host.contains("*") && !host.startsWith("*.")) {
            throw new IllegalArgumentException("Wildcard certificate host must start with '*.' but was: " + host);
        }
        // TLS-F3: Initialize the SslContext BEFORE publishing to the map.
        // The ConcurrentHashMap is read by SNI lookups on Netty I/O threads;
        // if the entry is visible before init() completes, sslContext() returns
        // null and SslHandler throws NPE. Initializing first also ensures the
        // map is never polluted if init() throws SSLException.
        certificateKeyPair.init(this);
        certificateKeyPairMap.put(host, certificateKeyPair);
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
        reloadLock.readLock().lock();
        try {
            if (fqdn == null || fqdn.isEmpty()) {
                CertificateKeyPair defaultPair = defaultMapping();
                if (defaultPair == null) {
                    throw new IllegalStateException("No TLS mapping found for hostname: " + fqdn + " and no default mapping configured");
                }
                return defaultPair;
            }

            try {
                CertificateKeyPair certificateKeyPair = certificateKeyPairMap.get(fqdn);

                // If `null` then it means mapping was not found with FQDN.
                // We'll try wildcard now.
                if (certificateKeyPair == null) {
                    int dotIndex = fqdn.indexOf('.');
                    // A wildcard cert (e.g. *.example.com) can never match a single-label
                    // hostname without a dot (e.g. "localhost"), so skip wildcard matching.
                    if (dotIndex != -1) {
                        fqdn = '*' + fqdn.substring(dotIndex);
                        certificateKeyPair = certificateKeyPairMap.get(fqdn);
                    }
                    if (certificateKeyPair != null) {
                        return certificateKeyPair;
                    } else {
                        // TLS-03: If no exact match, no wildcard match, and no default mapping,
                        // throw a clear error instead of returning null. A null return here
                        // causes a NullPointerException deep in the SslHandler with no
                        // indication of which hostname was missing.
                        CertificateKeyPair defaultPair = defaultMapping();
                        if (defaultPair == null) {
                            throw new IllegalStateException("No TLS mapping found for hostname: " + fqdn + " and no default mapping configured");
                        }
                        return defaultPair;
                    }
                } else {
                    return certificateKeyPair;
                }
            } catch (IllegalStateException ex) {
                // TLS-03: Let our explicit "no mapping" error propagate without wrapping.
                throw ex;
            } catch (Exception ex) {
                throw new IllegalStateException("Error resolving mapping for Hostname: " + fqdn, ex);
            }
        } finally {
            reloadLock.readLock().unlock();
        }
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
     * Accept all certificates (including self-signed certificates).
     * <p>
     * P0-4/SEC-11: When enabled in a PRODUCTION environment, a loud warning is
     * logged because accepting all certificates disables TLS chain-of-trust
     * verification, making the connection vulnerable to MITM attacks. This is
     * intended only for development and testing with self-signed certificates.
     */
    public TlsConfiguration setAcceptAllCerts(boolean acceptAllCerts) {
        if (acceptAllCerts) {
            logger.warn("SEC-11: acceptAllCerts is enabled. TLS certificate verification is DISABLED. "
                    + "This is a security risk in production — connections are vulnerable to MITM attacks. "
                    + "Only use this for development/testing with self-signed certificates.");
        }
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
     * Reload certificates for all mappings. This rebuilds the SslContext for each
     * CertificateKeyPair without dropping existing connections.
     *
     * <p>Existing connections continue using their already-negotiated TLS sessions.
     * Only new connections will use the updated certificates.</p>
     *
     * @return the number of successfully reloaded certificate mappings
     */
    public int reloadCertificates() {
        reloadLock.writeLock().lock();
        try {
            int reloaded = 0;
            for (Map.Entry<String, CertificateKeyPair> entry : certificateKeyPairMap.entrySet()) {
                try {
                    entry.getValue().init(this);
                    reloaded++;
                    logger.info("Successfully reloaded TLS certificate for host: {}", entry.getKey());
                } catch (SSLException ex) {
                    logger.error("Failed to reload TLS certificate for host: {}", entry.getKey(), ex);
                }
            }
            logger.info("Certificate reload completed: {}/{} mappings reloaded successfully",
                    reloaded, certificateKeyPairMap.size());
            return reloaded;
        } finally {
            reloadLock.writeLock().unlock();
        }
    }

    /**
     * Validate all parameters of this configuration
     *
     * @return this class instance
     * @throws IllegalArgumentException If any value is invalid
     * @throws NullPointerException     If any value is null
     */
    @Override
    public TlsConfiguration validate() {
        ListUtil.checkNonEmpty(ciphers, "Ciphers");
        ListUtil.checkNonEmpty(protocols, "Protocols");
        Objects.requireNonNull(mutualTLS, "MutualTLS");
        NumberUtil.checkZeroOrPositive(sessionTimeout, "Session Timeout");
        NumberUtil.checkZeroOrPositive(sessionCacheSize, "Session Cache Size");
        enabled();

        // MED-23: StartTLS is only meaningful for SMTP, IMAP, POP3 etc. -- not HTTP.
        // Since ExpressGateway is primarily an HTTP load balancer, warn if StartTLS is enabled.
        if (useStartTLS) {
            logger.warn("StartTLS is enabled. StartTLS is intended for protocols like SMTP/IMAP, not HTTP. " +
                    "Ensure this is intentional for your deployment protocol.");
        }

        // TLS-F5: Warn when deprecated ciphers (anonymous DH/ECDH, non-PFS RSA, SRP)
        // are configured. These ciphers are @Deprecated in the Cipher enum because they
        // are vulnerable to MITM attacks (anonymous key exchange) or lack forward secrecy
        // (static RSA key exchange). A compromised server key lets an attacker decrypt all
        // past traffic captured with non-PFS ciphers. We log a warning rather than
        // rejecting outright to allow intentional use in test/legacy environments.
        for (Cipher cipher : ciphers) {
            if (cipher.isDeprecated()) {
                logger.warn("TLS-F5: Deprecated cipher suite configured: {}. "
                        + "This cipher lacks forward secrecy or uses anonymous key exchange, "
                        + "making it vulnerable to passive decryption or MITM attacks. "
                        + "Consider removing it and using ECDHE or DHE-based cipher suites instead.", cipher.name());
            }
        }

        // Production environment security hardening (P0-4/SEC-11, HIGH-15, HIGH-16)
        Environment environment = resolveEnvironment();
        if (environment == Environment.PRODUCTION) {
            // HIGH-15: Reject deprecated TLS protocols in production (RFC 8996)
            for (Protocol protocol : protocols) {
                if (protocol == Protocol.TLS_1_1) {
                    throw new IllegalArgumentException(
                            "TLS 1.1 is deprecated per RFC 8996 and must not be used in PRODUCTION environment");
                }
            }

            // P0-4/SEC-11: Reject acceptAllCerts in PRODUCTION environments.
            // In production, disabling certificate verification exposes all backend
            // connections to MITM attacks. We throw here rather than just logging
            // because a misconfigured proxy that trusts any certificate is a critical
            // security vulnerability that must not reach production.
            if (acceptAllCerts) {
                throw new IllegalArgumentException(
                        "acceptAllCerts must not be enabled in PRODUCTION environment");
            }

            // HIGH-16: Reject insecure ciphers in production
            for (Cipher cipher : ciphers) {
                String name = cipher.name();
                if (name.contains("DH_anon") || name.contains("ECDH_anon")) {
                    throw new IllegalArgumentException(
                            "Anonymous DH/ECDH cipher " + name + " must not be used in PRODUCTION (no authentication)");
                }
                if (name.startsWith("TLS_SRP_")) {
                    throw new IllegalArgumentException(
                            "SRP cipher " + name + " must not be used in PRODUCTION environment");
                }
                if (name.startsWith("TLS_RSA_WITH_")) {
                    throw new IllegalArgumentException(
                            "Non-PFS RSA cipher " + name + " must not be used in PRODUCTION (no forward secrecy)");
                }

                // MED-20: Warn about CBC ciphers in production (POODLE vulnerability variants)
                if (name.contains("_CBC_")) {
                    logger.warn("CBC cipher {} is selected in PRODUCTION. Prefer GCM or ChaCha20 ciphers " +
                            "to avoid padding oracle vulnerabilities (POODLE variants).", name);
                }
            }
        }

        return this;
    }

    /**
     * Resolve the current environment, returning DEVELOPMENT if unavailable.
     */
    private static Environment resolveEnvironment() {
        try {
            ExpressGateway instance = ExpressGateway.getInstance();
            if (instance != null && instance.environment() != null) {
                return instance.environment();
            }
        } catch (Exception e) {
            // Environment not yet initialized (e.g., during tests)
        }
        return Environment.DEVELOPMENT;
    }

    /**
     * Atomically reload all certificate mappings by re-initializing SslContexts.
     * Acquires the write lock to ensure no concurrent reads see a partially-swapped state.
     *
     * @throws SSLException if any certificate re-initialization fails
     */
    public void reload() throws SSLException {
        reloadLock.writeLock().lock();
        try {
            for (Map.Entry<String, CertificateKeyPair> entry : certificateKeyPairMap.entrySet()) {
                entry.getValue().init(this);
            }
            logger.info("TLS configuration reloaded successfully for {} mapping(s)", certificateKeyPairMap.size());
        } finally {
            reloadLock.writeLock().unlock();
        }
    }

    /**
     * Get the reload lock for use by consumers that need to protect SslContext reads
     * during hot reload operations.
     */
    public ReentrantReadWriteLock reloadLock() {
        return reloadLock;
    }
}
