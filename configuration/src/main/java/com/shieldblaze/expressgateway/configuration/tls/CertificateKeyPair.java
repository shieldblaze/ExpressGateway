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

import com.shieldblaze.expressgateway.common.annotation.InternalCall;
import com.shieldblaze.expressgateway.common.crypto.Keypair;
import com.shieldblaze.expressgateway.common.utils.ListUtil;
import com.shieldblaze.expressgateway.concurrent.GlobalExecutors;
import io.netty.handler.ssl.ApplicationProtocolConfig;
import io.netty.handler.ssl.ApplicationProtocolNames;
import io.netty.handler.ssl.OpenSsl;
import io.netty.handler.ssl.SslContext;
import io.netty.handler.ssl.SslContextBuilder;
import io.netty.handler.ssl.SslProvider;
import io.netty.handler.ssl.util.InsecureTrustManagerFactory;
import io.netty.util.ReferenceCountUtil;
import org.apache.logging.log4j.LogManager;
import org.apache.logging.log4j.Logger;
import org.bouncycastle.cert.ocsp.BasicOCSPResp;
import org.bouncycastle.cert.ocsp.OCSPResp;
import org.bouncycastle.cert.ocsp.SingleResp;

import javax.net.ssl.SSLException;
import javax.net.ssl.TrustManagerFactory;
import java.io.ByteArrayInputStream;
import java.io.Closeable;
import java.io.IOException;
import java.security.KeyStore;
import java.security.PrivateKey;
import java.security.cert.Certificate;
import java.security.cert.CertificateException;
import java.security.cert.CertificateFactory;
import java.security.cert.X509Certificate;
import java.time.Duration;
import java.time.Instant;
import java.util.ArrayList;
import java.util.Arrays;
import java.util.Date;
import java.util.List;
import java.util.Objects;
import java.util.concurrent.ScheduledFuture;
import java.util.concurrent.TimeUnit;

/**
 * <p> {@link X509Certificate} and {@link PrivateKey} Pair </p>
 */
public final class CertificateKeyPair implements Runnable, Closeable {

    private static final Logger logger = LogManager.getLogger(CertificateKeyPair.class);

    private final List<X509Certificate> certificates = new ArrayList<>();
    private final PrivateKey privateKey;
    private final boolean useOCSPStapling;

    // TLS-01: Must be volatile — updated by the OCSP refresh scheduled task running
    // on GlobalExecutors and read by Netty I/O threads in the SslHandler callback.
    // Without volatile, I/O threads may read stale/partially-written data.
    private volatile byte[] ocspStaplingData;
    private ScheduledFuture<?> scheduledFuture;
    // TLS-F1: Must be volatile — init() builds the SslContext on an application thread
    // and sslContext() is read from Netty I/O threads during TLS handshake setup.
    // Without volatile, the I/O thread may see null or a partially-constructed SslContext
    // due to JMM reordering of the constructor and field assignment.
    private volatile SslContext sslContext;

    /**
     * Configurable ALPN protocol list. Defaults to [h2, http/1.1].
     */
    private List<String> alpnProtocols = Arrays.asList(
            ApplicationProtocolNames.HTTP_2,
            ApplicationProtocolNames.HTTP_1_1
    );

    /**
     * <p> Create a new TLS Client Instance </p>
     * This should be used when there is no need of Mutual TLS handshake.
     * This constructor does not use {@link X509Certificate} or {@link PrivateKey}
     */
    public static CertificateKeyPair newDefaultClientInstance() {
        return new CertificateKeyPair();
    }

    /**
     * Create a new TLS Client Instance
     *
     * @param x509Certificates {@link List} of {@link X509Certificate} certificates
     * @param privateKey       {@link PrivateKey} of certificate
     * @return {@link CertificateKeyPair} Instance
     */
    public static CertificateKeyPair forClient(List<X509Certificate> x509Certificates, PrivateKey privateKey) {
        return new CertificateKeyPair(x509Certificates, privateKey);
    }

    /**
     * Create a new TLS Server Instance
     *
     * @param x509Certificates {@link List} of {@link X509Certificate} certificates
     * @param privateKey       {@link PrivateKey} of certificate
     * @param useOCSPStapling  Set to {@code true} to enable OCSP Stapling else {@code false}
     * @return {@link CertificateKeyPair} Instance
     */
    public static CertificateKeyPair forServer(List<X509Certificate> x509Certificates, PrivateKey privateKey, boolean useOCSPStapling) {
        return new CertificateKeyPair(x509Certificates, privateKey, useOCSPStapling);
    }

    /**
     * Create a new TLS Server Instance
     *
     * @param x509Certificates {@link X509Certificate} certificate chain
     * @param privateKey       {@link PrivateKey} of certificate
     * @param useOCSPStapling  Set to {@code true} to enable OCSP Stapling else {@code false}
     * @return {@link CertificateKeyPair} Instance
     */
    public static CertificateKeyPair forServer(String x509Certificates, String privateKey, boolean useOCSPStapling) throws IOException {
        return new CertificateKeyPair(x509Certificates, privateKey, useOCSPStapling);
    }

    /**
     * @see #newDefaultClientInstance()
     */
    private CertificateKeyPair() {
        privateKey = null;
        useOCSPStapling = false;
    }

    private CertificateKeyPair(List<X509Certificate> x509Certificates, PrivateKey privateKey) {
        this(x509Certificates, privateKey, false);
    }

    private CertificateKeyPair(List<X509Certificate> x509Certificates, PrivateKey privateKey, boolean useOCSPStapling) {
        ListUtil.checkNonEmpty(x509Certificates, "X509Certificates");
        Objects.requireNonNull(privateKey, "Private Key");

        certificates.addAll(x509Certificates);
        this.privateKey = privateKey;
        this.useOCSPStapling = useOCSPStapling;

        // SEC-08: Check certificate expiration at construction time.
        checkCertificateExpiration();
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
            logger.error(e);
            throw new IllegalArgumentException("Error Occurred: " + e);
        }

        // SEC-08: Check certificate expiration at construction time.
        checkCertificateExpiration();
    }

    /**
     * SEC-08: Check if any certificate in the chain expires within 30 days and log a warning.
     * Expired certificates cause TLS handshake failures. Near-expiration warnings give
     * operators time to rotate certificates before outages occur. This mirrors the
     * approach used by Nginx (ssl_certificate_expiry_warning) and HAProxy (cert-expiry).
     */
    private void checkCertificateExpiration() {
        Instant now = Instant.now();
        Duration warningThreshold = Duration.ofDays(30);

        for (X509Certificate cert : certificates) {
            Date notAfter = cert.getNotAfter();
            if (notAfter == null) {
                continue;
            }

            Instant expiry = notAfter.toInstant();
            Duration remaining = Duration.between(now, expiry);

            String subject = cert.getSubjectX500Principal().getName();

            if (remaining.isNegative()) {
                logger.error("Certificate has EXPIRED: subject='{}', expired on {}. " +
                        "TLS handshakes will fail until the certificate is replaced.", subject, notAfter);
            } else if (remaining.compareTo(warningThreshold) < 0) {
                long daysRemaining = remaining.toDays();
                logger.warn("Certificate expiring soon: subject='{}', expires on {} ({} days remaining). " +
                        "Rotate the certificate to avoid service disruption.", subject, notAfter, daysRemaining);
            }
        }
    }

    /**
     * <p> Initialize and build {@link SslContext} </p>
     * Internal Call; Initiated by {@link TlsConfiguration#addMapping(String, CertificateKeyPair)}
     *
     * @param tlsConfiguration {@link TlsConfiguration} to use for initializing and building.
     */
    @InternalCall
    public CertificateKeyPair init(TlsConfiguration tlsConfiguration) throws SSLException {
        if (useOCSPStapling && !OpenSsl.isOcspSupported()) {
            throw new IllegalArgumentException("OCSP Stapling is unavailable because OpenSSL is unavailable.");
        }

        List<Cipher> cipherList = new ArrayList<>(tlsConfiguration.ciphers());

        List<String> ciphers = new ArrayList<>();
        for (Cipher cipher : cipherList) {
            ciphers.add(cipher.toString());
        }

        // TLS-05: Prefer FATAL_ALERT per RFC 7301 — if the client's ALPN list does not
        // contain any protocol the server supports, the connection should be terminated
        // with a fatal alert rather than silently accepting an unsupported protocol.
        // However, OpenSSL's ALPN implementation does not support FATAL_ALERT behavior,
        // so we fall back to ACCEPT when using the OpenSSL provider.
        SslProvider sslProvider = OpenSsl.isAvailable() ? SslProvider.OPENSSL : SslProvider.JDK;
        ApplicationProtocolConfig.SelectedListenerFailureBehavior alpnFailureBehavior =
                sslProvider == SslProvider.JDK
                        ? ApplicationProtocolConfig.SelectedListenerFailureBehavior.FATAL_ALERT
                        : ApplicationProtocolConfig.SelectedListenerFailureBehavior.ACCEPT;

        SslContextBuilder sslContextBuilder;
        if (tlsConfiguration instanceof TlsServerConfiguration serverConfig) {
            sslContextBuilder = SslContextBuilder.forServer(privateKey, certificates)
                    .sslProvider(sslProvider)
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
                            alpnFailureBehavior,
                            alpnProtocols));

            // BUG-009 fix: When mTLS is REQUIRED or OPTIONAL, configure the trust manager
            // so the server can validate client certificates against a custom CA store.
            if (tlsConfiguration.mutualTLS() == MutualTLS.REQUIRED || tlsConfiguration.mutualTLS() == MutualTLS.OPTIONAL) {
                sslContextBuilder.trustManager(serverConfig.buildTrustManagerFactory());
            }

            if (useOCSPStapling) {
                scheduledFuture = GlobalExecutors.submitTaskAndRunEvery(this, 0, 6, TimeUnit.HOURS);
            }
        } else {
            TrustManagerFactory trustManagerFactory;
            if (tlsConfiguration.acceptAllCerts()) {
                trustManagerFactory = InsecureTrustManagerFactory.INSTANCE;
            } else {
                try {
                    trustManagerFactory = TrustManagerFactory.getInstance(TrustManagerFactory.getDefaultAlgorithm());
                    trustManagerFactory.init((KeyStore) null);
                } catch (Exception ex) {
                    // This should never happen
                    Error error = new Error(ex);
                    logger.fatal(error);
                    throw error;
                }
            }

            // BUG-016: Do NOT set clientAuth on a client-side SslContext. The clientAuth
            // setting is a server-side concept (controls whether the server requests a
            // client certificate). On a client SslContext:
            //   - JDK provider: silently ignored.
            //   - OpenSSL/BoringSSL provider: can cause SSL_CTX_set_verify to set
            //     SSL_VERIFY_FAIL_IF_NO_PEER_CERT on the client side, which makes
            //     the OpenSSL engine treat the server's certificate presentation as
            //     a client-auth requirement and can produce spurious
            //     PEER_DID_NOT_RETURN_A_CERTIFICATE errors during handshake.
            sslContextBuilder = SslContextBuilder.forClient()
                    .sslProvider(sslProvider)
                    .protocols(Protocol.getProtocols(tlsConfiguration.protocols()))
                    .ciphers(ciphers)
                    .trustManager(trustManagerFactory)
                    .startTls(tlsConfiguration.useStartTLS())
                    .applicationProtocolConfig(new ApplicationProtocolConfig(
                            ApplicationProtocolConfig.Protocol.ALPN,
                            ApplicationProtocolConfig.SelectorFailureBehavior.NO_ADVERTISE,
                            alpnFailureBehavior,
                            alpnProtocols));

            if (tlsConfiguration.mutualTLS() == MutualTLS.REQUIRED || tlsConfiguration.mutualTLS() == MutualTLS.OPTIONAL) {
                sslContextBuilder.keyManager(privateKey, certificates);
            }
        }

        sslContext = sslContextBuilder.build();
        return this;
    }

    public SslContext sslContext() {
        return sslContext;
    }

    /**
     * Get the {@link PrivateKey} associated with this certificate pair.
     *
     * <p>Required by QUIC-TLS (RFC 9001) which uses BoringSSL natively and cannot
     * reuse a pre-built Netty {@link SslContext}. The QUIC SSL context builder
     * ({@code QuicSslContextBuilder.forServer}) requires the raw private key and
     * certificate chain directly.</p>
     *
     * @return the private key, or {@code null} for default client instances
     */
    public PrivateKey privateKey() {
        return privateKey;
    }

    /**
     * Get the {@link X509Certificate} chain associated with this certificate pair.
     *
     * <p>Returns an unmodifiable view of the certificate chain. Required by QUIC-TLS
     * (RFC 9001) which uses BoringSSL natively and cannot reuse a pre-built Netty
     * {@link SslContext}.</p>
     *
     * @return unmodifiable list of certificates in the chain
     */
    public List<X509Certificate> certificates() {
        return List.copyOf(certificates);
    }

    /**
     * TLS-F2: Return a defensive copy of the OCSP stapling data.
     * The internal array is written by the OCSP refresh task and read by Netty I/O threads.
     * Without copying, a caller could mutate the array contents, corrupting the stapled
     * response sent to clients during the TLS handshake.
     */
    public byte[] ocspStaplingData() {
        byte[] data = ocspStaplingData;
        return data != null ? data.clone() : null;
    }

    public boolean useOCSPStapling() {
        return useOCSPStapling;
    }

    /**
     * Get the configured ALPN protocol list.
     */
    public List<String> alpnProtocols() {
        return alpnProtocols;
    }

    /**
     * Set the ALPN protocol list. Defaults to [h2, http/1.1].
     *
     * @param alpnProtocols list of ALPN protocol names
     * @return this instance
     */
    public CertificateKeyPair setAlpnProtocols(List<String> alpnProtocols) {
        Objects.requireNonNull(alpnProtocols, "alpnProtocols");
        if (alpnProtocols.isEmpty()) {
            throw new IllegalArgumentException("alpnProtocols must not be empty");
        }
        this.alpnProtocols = alpnProtocols;
        return this;
    }

    @Override
    public void run() {
        try {
            // TLS-04: Guard against certificate chains with fewer than 2 certificates.
            // OCSP stapling requires both the end-entity certificate and the issuer
            // certificate. A self-signed cert or incomplete chain would cause an
            // IndexOutOfBoundsException here.
            if (certificates.size() < 2) {
                logger.warn("OCSP stapling requires at least 2 certificates in chain (end-entity + issuer), " +
                        "but chain has {} certificate(s). Skipping OCSP refresh.", certificates.size());
                ocspStaplingData = null;
                return;
            }
            OCSPResp response = OCSPClient.response(certificates.get(0), certificates.get(1));
            SingleResp ocspResp = ((BasicOCSPResp) response.getResponseObject()).getResponses()[0];

            // null indicates good status.
            if (ocspResp.getCertStatus() == null) {
                // TLS-F2: getEncoded() returns a fresh array each call, so this assignment
                // is already a safe snapshot. The defensive copy is on the getter side.
                ocspStaplingData = response.getEncoded();
                return;
            }
        } catch (Exception ex) {
            logger.error("OCSP stapling refresh failed; stapling data will be cleared", ex);
        }
        // MED-22: Log when OCSP stapling data is nullified so operators have visibility
        if (ocspStaplingData != null) {
            logger.warn("OCSP stapling data cleared for certificate CN={}; TLS clients will not receive stapled response",
                    certificates.get(0).getSubjectX500Principal().getName());
        }
        ocspStaplingData = null;
    }

    @Override
    public void close() throws IOException {
        if (scheduledFuture != null && !scheduledFuture.isCancelled()) {
            scheduledFuture.cancel(true);
        }

        // TLS-F7: When using the OpenSSL provider, SslContext implements ReferenceCounted
        // and holds a native SSL_CTX pointer. If not explicitly released, the native memory
        // is leaked until GC finalizes the object (if ever). With the JDK provider,
        // ReferenceCountUtil.release() is a safe no-op since JdkSslContext does not
        // implement ReferenceCounted.
        if (sslContext != null) {
            ReferenceCountUtil.release(sslContext);
            sslContext = null;
        }
    }
}
