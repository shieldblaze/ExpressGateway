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
import org.apache.logging.log4j.LogManager;
import org.apache.logging.log4j.Logger;

import javax.net.ssl.TrustManagerFactory;
import java.io.File;
import java.io.FileInputStream;
import java.nio.file.Files;
import java.nio.file.Path;
import java.security.KeyStore;
import java.security.cert.Certificate;
import java.security.cert.CertificateFactory;
import java.security.cert.X509CRL;
import java.security.cert.X509Certificate;
import java.util.Collection;

/**
 * Configuration for TLS Server (Internet--to--ExpressGateway)
 */
public final class TlsServerConfiguration extends TlsConfiguration {

    private static final Logger logger = LogManager.getLogger(TlsServerConfiguration.class);

    @JsonIgnore
    private boolean validated;

    /**
     * Path to a CRL (Certificate Revocation List) file in PEM or DER format.
     * When set and mTLS is enabled, client certificates will be checked against this CRL.
     */
    @JsonProperty("crlFile")
    private String crlFile;

    /**
     * Optional trust certificate file for mTLS client certificate validation.
     * When {@link MutualTLS#REQUIRED} or {@link MutualTLS#OPTIONAL} is set,
     * this file provides the trusted CA certificates used to verify client certificates.
     * If null, the JVM's default trust store is used.
     */
    @JsonProperty("trustCertificateFile")
    private File trustCertificateFile;

    /**
     * This is the default implementation of {@link TlsClientConfiguration}
     * which is disabled by default.
     * </p>
     * <p>
     * To enable this, call {@link #enabled()}.
     */
    @JsonIgnore
    public static final TlsServerConfiguration DEFAULT = new TlsServerConfiguration();

    static {
        DEFAULT.ciphers = IntermediateCrypto.CIPHERS;
        DEFAULT.protocols = IntermediateCrypto.PROTOCOLS;
        DEFAULT.useStartTLS = false;
        DEFAULT.sessionTimeout = 43_200;
        DEFAULT.sessionCacheSize = 1_000_000;
        DEFAULT.validated = true;
    }

    /**
     * Set the trust certificate file for mTLS client certificate validation.
     *
     * @param trustCertificateFile PEM file containing trusted CA certificates
     * @return this instance for chaining
     */
    public TlsServerConfiguration setTrustCertificateFile(File trustCertificateFile) {
        this.trustCertificateFile = trustCertificateFile;
        return this;
    }

    /**
     * Get the trust certificate file for mTLS client certificate validation.
     */
    public File trustCertificateFile() {
        return trustCertificateFile;
    }

    /**
     * Build a {@link TrustManagerFactory} from the configured trust certificate file.
     * If no file is set, returns a factory initialized with the JVM default trust store.
     */
    TrustManagerFactory buildTrustManagerFactory() {
        try {
            TrustManagerFactory tmf = TrustManagerFactory.getInstance(TrustManagerFactory.getDefaultAlgorithm());
            if (trustCertificateFile != null && trustCertificateFile.exists()) {
                KeyStore trustStore = KeyStore.getInstance(KeyStore.getDefaultType());
                trustStore.load(null, null);
                CertificateFactory cf = CertificateFactory.getInstance("X.509");
                try (FileInputStream fis = new FileInputStream(trustCertificateFile)) {
                    Collection<? extends Certificate> certs = cf.generateCertificates(fis);
                    int i = 0;
                    for (Certificate cert : certs) {
                        trustStore.setCertificateEntry("trust-" + i++, cert);
                    }
                }
                tmf.init(trustStore);
            } else {
                tmf.init((KeyStore) null); // JVM default trust store
            }
            return tmf;
        } catch (Exception ex) {
            throw new RuntimeException("Failed to build TrustManagerFactory for mTLS", ex);
        }
    }

    @Override
    public TlsConfiguration validate() {
        super.validate();

        // MED-46: Validate trustCertificateFile exists if set
        if (trustCertificateFile != null && !trustCertificateFile.exists()) {
            throw new IllegalArgumentException("trustCertificateFile does not exist: " + trustCertificateFile);
        }

        // HIGH-17: Validate CRL file exists if set
        if (crlFile != null && !crlFile.isEmpty()) {
            if (!Files.exists(Path.of(crlFile))) {
                throw new IllegalArgumentException("CRL file does not exist: " + crlFile);
            }
            logger.info("CRL checking enabled with file: {}", crlFile);
        }

        validated = true;
        return this;
    }

    /**
     * Get the CRL file path
     */
    public String crlFile() {
        return crlFile;
    }

    /**
     * Set the CRL file path for client certificate revocation checking
     */
    public TlsServerConfiguration setCrlFile(String crlFile) {
        this.crlFile = crlFile;
        return this;
    }

    /**
     * Set the trust certificate file for mTLS using a file path string.
     *
     * @param trustCertificateFile path to the PEM file containing trusted CA certificates
     * @return this instance for chaining
     */
    public TlsServerConfiguration setTrustCertificateFile(String trustCertificateFile) {
        this.trustCertificateFile = trustCertificateFile != null ? new File(trustCertificateFile) : null;
        return this;
    }

    /**
     * Load the CRL from the configured file path.
     *
     * @return the loaded {@link X509CRL}, or {@code null} if no CRL file is configured
     * @throws Exception if the CRL file cannot be read or parsed
     */
    public X509CRL loadCRL() throws Exception {
        if (crlFile == null || crlFile.isEmpty()) {
            return null;
        }
        CertificateFactory cf = CertificateFactory.getInstance("X.509");
        try (FileInputStream fis = new FileInputStream(crlFile)) {
            return (X509CRL) cf.generateCRL(fis);
        }
    }

    @Override
    public boolean validated() {
        return validated;
    }

    public static TlsServerConfiguration copyFrom(TlsServerConfiguration from) {
        from.validate();

        TlsServerConfiguration configuration = new TlsServerConfiguration();
        configuration.certificateKeyPairMap.putAll(from.certificateKeyPairMap);

        configuration.ciphers = from.ciphers;
        configuration.protocols = from.protocols;
        configuration.mutualTLS = from.mutualTLS;
        configuration.useStartTLS = from.useStartTLS;
        configuration.sessionTimeout = from.sessionTimeout;
        configuration.sessionCacheSize = from.sessionCacheSize;
        configuration.acceptAllCerts = from.acceptAllCerts;
        configuration.crlFile = from.crlFile;
        configuration.trustCertificateFile = from.trustCertificateFile;

        configuration.validate();
        return configuration;
    }
}
