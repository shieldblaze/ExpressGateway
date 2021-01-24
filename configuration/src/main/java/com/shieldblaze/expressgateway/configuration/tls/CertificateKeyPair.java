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

import com.shieldblaze.expressgateway.common.crypto.Keypair;
import com.shieldblaze.expressgateway.concurrent.GlobalExecutors;
import io.netty.handler.ssl.ApplicationProtocolConfig;
import io.netty.handler.ssl.ApplicationProtocolNames;
import io.netty.handler.ssl.OpenSsl;
import io.netty.handler.ssl.SslContext;
import io.netty.handler.ssl.SslContextBuilder;
import io.netty.handler.ssl.SslProvider;
import io.netty.handler.ssl.util.InsecureTrustManagerFactory;
import org.bouncycastle.cert.ocsp.BasicOCSPResp;
import org.bouncycastle.cert.ocsp.OCSPResp;
import org.bouncycastle.cert.ocsp.SingleResp;

import javax.net.ssl.SSLException;
import javax.net.ssl.TrustManagerFactory;
import java.io.ByteArrayInputStream;
import java.io.Closeable;
import java.io.IOException;
import java.security.KeyStore;
import java.security.KeyStoreException;
import java.security.NoSuchAlgorithmException;
import java.security.PrivateKey;
import java.security.cert.Certificate;
import java.security.cert.CertificateException;
import java.security.cert.CertificateFactory;
import java.security.cert.X509Certificate;
import java.util.ArrayList;
import java.util.List;
import java.util.concurrent.ScheduledFuture;
import java.util.concurrent.TimeUnit;

/**
 * {@link X509Certificate} and {@link PrivateKey} Pair
 */
public final class CertificateKeyPair implements Runnable, Closeable {

    private final List<X509Certificate> certificates = new ArrayList<>();
    private final PrivateKey privateKey;
    private final boolean useOCSPStapling;

    private volatile byte[] ocspStaplingData = null;
    private ScheduledFuture<?> scheduledFuture;
    private SslContext sslContext;

    private boolean noCertKey = false;

    /**
     * <p> TLS for Client </p>
     * Should be used when there is no need of Mutual TLS handshake.
     */
    public CertificateKeyPair() {
        privateKey = null;
        useOCSPStapling = false;
        noCertKey = true;
    }

    /**
     * TLS for Client / Server with OCSPStapling disabled.
     *
     * @param x509Certificates {@link List} of {@link X509Certificate} certificates
     * @param privateKey       {@link PrivateKey} Instance
     */
    public CertificateKeyPair(List<X509Certificate> x509Certificates, PrivateKey privateKey) {
        this(x509Certificates, privateKey, false);
    }

    /**
     * TLS for Client / Server
     *
     * @param x509Certificates {@link List} of {@link X509Certificate} certificates
     * @param privateKey       {@link PrivateKey} Instance
     * @param useOCSPStapling  Set to {@code true} to enable OCSP Stapling.
     */
    public CertificateKeyPair(List<X509Certificate> x509Certificates, PrivateKey privateKey, boolean useOCSPStapling) {
        certificates.addAll(x509Certificates);
        this.privateKey = privateKey;
        this.useOCSPStapling = useOCSPStapling;
    }

    /**
     * TLS for Client / Server
     *
     * @param certificateChain Certificate Chain in PEM format
     * @param privateKey       Private Key in PEM format
     * @param useOCSPStapling  Set to {@code true} to enable OCSP Stapling.
     * @throws IOException If an error occurred while parsing CertificateChain
     */
    public CertificateKeyPair(String certificateChain, String privateKey, boolean useOCSPStapling) throws IOException {
        this.privateKey = Keypair.parse(privateKey);
        this.useOCSPStapling = useOCSPStapling;

        try (ByteArrayInputStream inputStream = new ByteArrayInputStream(certificateChain.getBytes())) {
            CertificateFactory cf = CertificateFactory.getInstance("X.509");
            for (Certificate certificate : cf.generateCertificates(inputStream)) {
                certificates.add((X509Certificate) certificate);
            }
        } catch (IOException | CertificateException e) {
            throw new IllegalArgumentException("Error Occurred: " + e);
        }
    }

    /**
     * Initialize and build {@link SslContext}
     *
     * @param tlsConfiguration {@link TLSConfiguration} to use for initializing and building.
     */
    public CertificateKeyPair init(TLSConfiguration tlsConfiguration) throws NoSuchAlgorithmException, KeyStoreException, SSLException {
        if (useOCSPStapling && !OpenSsl.isOcspSupported()) {
            throw new IllegalArgumentException("OCSP Stapling is unavailable because OpenSSL is unavailable.");
        }

        List<String> ciphers = new ArrayList<>();
        for (Cipher cipher : tlsConfiguration.ciphers()) {
            ciphers.add(cipher.name());
        }

        SslContextBuilder sslContextBuilder;
        if (tlsConfiguration.forServer()) {
            sslContextBuilder = SslContextBuilder.forServer(privateKey, certificates)
                    .sslProvider(OpenSsl.isAvailable() ? SslProvider.OPENSSL : SslProvider.JDK)
                    .protocols(Protocol.getProtocols(tlsConfiguration.protocols()))
                    .ciphers(ciphers)
                    .enableOcsp(useOCSPStapling)
                    .clientAuth(tlsConfiguration.mutualTLS().clientAuth())
                    .startTls(tlsConfiguration.useStartTLS())
                    .sessionTimeout(tlsConfiguration.sessionTimeout())
                    .sessionCacheSize(tlsConfiguration.sessionCacheSize())
                    .applicationProtocolConfig(new ApplicationProtocolConfig(
                            ApplicationProtocolConfig.Protocol.ALPN,
                            ApplicationProtocolConfig.SelectorFailureBehavior.NO_ADVERTISE,
                            ApplicationProtocolConfig.SelectedListenerFailureBehavior.ACCEPT,
                            ApplicationProtocolNames.HTTP_2,
                            ApplicationProtocolNames.HTTP_1_1));

            if (useOCSPStapling) {
                scheduledFuture = GlobalExecutors.INSTANCE.submitTaskAndRunEvery(this, 0, 6, TimeUnit.HOURS);
            }
        } else {
            TrustManagerFactory trustManagerFactory;
            if (tlsConfiguration.acceptAllCerts()) {
                trustManagerFactory = InsecureTrustManagerFactory.INSTANCE;
            } else {
                trustManagerFactory = TrustManagerFactory.getInstance(TrustManagerFactory.getDefaultAlgorithm());
                trustManagerFactory.init((KeyStore) null);
            }

            sslContextBuilder = SslContextBuilder.forClient()
                    .sslProvider(OpenSsl.isAvailable() ? SslProvider.OPENSSL : SslProvider.JDK)
                    .protocols(Protocol.getProtocols(tlsConfiguration.protocols()))
                    .ciphers(ciphers)
                    .clientAuth(tlsConfiguration.mutualTLS().clientAuth())
                    .trustManager(trustManagerFactory)
                    .startTls(false)
                    .applicationProtocolConfig(new ApplicationProtocolConfig(
                            ApplicationProtocolConfig.Protocol.ALPN,
                            ApplicationProtocolConfig.SelectorFailureBehavior.NO_ADVERTISE,
                            ApplicationProtocolConfig.SelectedListenerFailureBehavior.ACCEPT,
                            ApplicationProtocolNames.HTTP_2,
                            ApplicationProtocolNames.HTTP_1_1));

            if (tlsConfiguration.mutualTLS() == MutualTLS.REQUIRED || tlsConfiguration.mutualTLS() == MutualTLS.OPTIONAL) {
                sslContextBuilder.keyManager(privateKey, certificates);
            }
        }

        this.sslContext = sslContextBuilder.build();
        return this;
    }

    public List<X509Certificate> certificates() {
        return certificates;
    }

    PrivateKey privateKey() {
        return privateKey;
    }

    public SslContext sslContext() {
        return sslContext;
    }

    public byte[] ocspStaplingData() {
        return ocspStaplingData;
    }

    public boolean useOCSPStapling() {
        return useOCSPStapling;
    }

    public boolean noCertKey() {
        return noCertKey;
    }

    @Override
    public void run() {
        try {
            OCSPResp response = OCSPClient.response(certificates.get(0), certificates.get(1));
            SingleResp ocspResp = ((BasicOCSPResp) response.getResponseObject()).getResponses()[0];
            if (ocspResp.getCertStatus() == null) {
                ocspStaplingData = response.getEncoded();
                return;
            }
        } catch (Exception ex) {
            // Ignore
        }
        ocspStaplingData = null;
    }

    @Override
    public void close() throws IOException {
        if (scheduledFuture != null && !scheduledFuture.isCancelled()) {
            scheduledFuture.cancel(true);
        }
    }
}
