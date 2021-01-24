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

import com.fasterxml.jackson.annotation.JsonProperty;
import com.shieldblaze.expressgateway.configuration.ConfigurationMarshaller;
import io.netty.util.internal.SystemPropertyUtil;
import org.apache.logging.log4j.LogManager;
import org.apache.logging.log4j.Logger;

import javax.net.ssl.SSLException;
import java.io.IOException;
import java.security.KeyStoreException;
import java.security.NoSuchAlgorithmException;
import java.util.List;
import java.util.Map;
import java.util.concurrent.ConcurrentHashMap;

/**
 * Configuration for TLS
 */
public final class TLSConfiguration extends ConfigurationMarshaller {
    private static final Logger logger = LogManager.getLogger(TLSConfiguration.class);

    private final Map<String, CertificateKeyPair> certificateKeyPairMap = new ConcurrentHashMap<>();

    @JsonProperty("forServer")
    private boolean forServer;

    @JsonProperty("ciphers")
    private List<Cipher> ciphers;

    @JsonProperty("protocols")
    private List<Protocol> protocols;

    @JsonProperty("mutualTLS")
    private MutualTLS mutualTLS = MutualTLS.NOT_REQUIRED;

    @JsonProperty("useStartTLS")
    private boolean useStartTLS;

    @JsonProperty("sessionTimeout")
    private int sessionTimeout;

    @JsonProperty("sessionCacheSize")
    private int sessionCacheSize;

    @JsonProperty("acceptAllCerts")
    private boolean acceptAllCerts;

    public static final TLSConfiguration DEFAULT_CLIENT = new TLSConfiguration();
    public static final TLSConfiguration DEFAULT_SERVER = new TLSConfiguration();

    static {
        // Default Client
        {
            DEFAULT_CLIENT.forServer = false;
            boolean useModernCrypto = SystemPropertyUtil.getBoolean("useModernCrypto", false);

            if (useModernCrypto) {
                DEFAULT_CLIENT.ciphers = ModernCrypto.CIPHERS;
                DEFAULT_CLIENT.protocols = ModernCrypto.PROTOCOLS;
            } else {
                DEFAULT_CLIENT.ciphers = IntermediateCrypto.CIPHERS;
                DEFAULT_CLIENT.protocols = IntermediateCrypto.PROTOCOLS;
            }

            DEFAULT_CLIENT.useStartTLS = false;
            DEFAULT_CLIENT.acceptAllCerts = false;
        }

        // Default Server
        {
            DEFAULT_SERVER.forServer = true;
            boolean useModernCrypto = SystemPropertyUtil.getBoolean("useModernCrypto", false);

            if (useModernCrypto) {
                DEFAULT_SERVER.ciphers = ModernCrypto.CIPHERS;
                DEFAULT_SERVER.protocols = ModernCrypto.PROTOCOLS;
            } else {
                DEFAULT_SERVER.ciphers = IntermediateCrypto.CIPHERS;
                DEFAULT_SERVER.protocols = IntermediateCrypto.PROTOCOLS;
            }

            DEFAULT_SERVER.useStartTLS = false;
            DEFAULT_SERVER.sessionTimeout = 43200;
            DEFAULT_SERVER.sessionCacheSize = 100_000;
        }
    }

    public Map<String, CertificateKeyPair> certificateKeyPairMap() {
        return certificateKeyPairMap;
    }

    /**
     * Add the default {@link CertificateKeyPair} mapping
     */
    public TLSConfiguration defaultMapping(CertificateKeyPair certificateKeyPair) throws NoSuchAlgorithmException, KeyStoreException, SSLException {
        addMapping("DEFAULT_HOST", certificateKeyPair);
        return this;
    }

    public TLSConfiguration addMapping(String host, CertificateKeyPair certificateKeyPair) throws NoSuchAlgorithmException, KeyStoreException, SSLException {
        certificateKeyPairMap.put(host, certificateKeyPair);
        if (!certificateKeyPair.noCertKey()) {
            certificateKeyPair.init(this);
        }
        return this;
    }

    public boolean removeMapping(String host) {
        return certificateKeyPairMap.remove(host) != null;
    }

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

    /**
     * Get the default mapping.
     */
    public CertificateKeyPair defaultMapping() {
        CertificateKeyPair certificateKeyPair = certificateKeyPairMap.get("DEFAULT_HOST");
        if (certificateKeyPair == null && !forServer) {
            try {
                certificateKeyPair = new CertificateKeyPair();
                certificateKeyPair.init(this);
                defaultMapping(certificateKeyPair);
            } catch (Exception ex) {
                logger.error("Caught error while initializing TLS Client DefaultMapping");
            }
        }
        return certificateKeyPair;
    }

    public boolean forServer() {
        return forServer;
    }

    TLSConfiguration forServer(boolean forServer) {
        this.forServer = forServer;
        return this;
    }

    public TLSConfiguration ciphers(List<Cipher> ciphers) {
        this.ciphers = ciphers;
        return this;
    }

    public TLSConfiguration protocols(List<Protocol> protocols) {
        this.protocols = protocols;
        return this;
    }

    public TLSConfiguration mutualTLS(MutualTLS mutualTLS) {
        this.mutualTLS = mutualTLS;
        return this;
    }

    public TLSConfiguration useStartTLS(boolean useStartTLS) {
        this.useStartTLS = useStartTLS;
        return this;
    }

    public TLSConfiguration sessionTimeout(int sessionTimeout) {
        this.sessionTimeout = sessionTimeout;
        return this;
    }

    public TLSConfiguration sessionCacheSize(int sessionCacheSize) {
        this.sessionCacheSize = sessionCacheSize;
        return this;
    }

    public TLSConfiguration acceptAllCerts(boolean acceptAllCerts) {
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

    public static TLSConfiguration loadClient() throws IOException {
        return loadFrom(TLSConfiguration.class, "TLSClient.yaml");
    }

    public void saveClient() throws IOException {
        saveTo(this, "TLSClient.yaml");
    }

    public static TLSConfiguration loadServer() throws IOException {
        return loadFrom(TLSConfiguration.class, "TLSServer.yaml");
    }

    public void saveServer() throws IOException {
        saveTo(this, "TLSServer.yaml");
    }
}
