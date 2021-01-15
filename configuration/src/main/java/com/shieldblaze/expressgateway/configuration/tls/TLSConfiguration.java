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

import com.google.gson.annotations.Expose;
import com.shieldblaze.expressgateway.configuration.ConfigurationMarshaller;
import org.apache.logging.log4j.LogManager;
import org.apache.logging.log4j.Logger;

import javax.net.ssl.SSLException;
import java.io.IOException;
import java.security.KeyStoreException;
import java.security.NoSuchAlgorithmException;
import java.util.List;
import java.util.Map;
import java.util.TreeMap;
import java.util.concurrent.ConcurrentHashMap;

/**
 * Configuration for TLS
 */
public final class TLSConfiguration extends ConfigurationMarshaller {
    private static final Logger logger = LogManager.getLogger(TLSConfiguration.class);

    private final Map<String, CertificateKeyPair> certificateKeyPairMap = new ConcurrentHashMap<>();

    @Expose
    private boolean forServer;

    @Expose
    private List<Cipher> ciphers;

    @Expose
    private List<Protocol> protocols;

    @Expose
    private MutualTLS mutualTLS;

    @Expose
    private boolean useStartTLS;

    @Expose
    private boolean useALPN;

    @Expose
    private int sessionTimeout;

    @Expose
    private int sessionCacheSize;

    @Expose
    private boolean acceptAllCerts;

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

    public TLSConfiguration useALPN(boolean useALPN) {
        this.useALPN = useALPN;
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

    public boolean useALPN() {
        return useALPN;
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

    public static TLSConfiguration loadFrom(String profileName, String password, boolean forServer) throws Exception {
        TLSConfiguration tlsConfiguration = loadFrom(TLSConfiguration.class, profileName, true, forServer ? "TLSServer.json" : "TLSClient.json");
        if (forServer) {
            KeyStoreHandler.loadClient(tlsConfiguration, profileName, password);
        } else {
            KeyStoreHandler.loadServer(tlsConfiguration, profileName, password);
        }
        return tlsConfiguration;
    }

    public void saveTo(String profileName, String password) throws Exception {
        saveTo(this, profileName, true, forServer ? "TLSServer.json" : "TLSClient.json");
        if (forServer) {
            KeyStoreHandler.saveServer(this, profileName, password);
        } else {
            KeyStoreHandler.saveClient(this, profileName, password);
        }
    }
}
