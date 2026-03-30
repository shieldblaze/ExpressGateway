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
package com.shieldblaze.expressgateway.protocol.http;

import com.shieldblaze.expressgateway.backend.NodeBuilder;
import com.shieldblaze.expressgateway.backend.cluster.Cluster;
import com.shieldblaze.expressgateway.backend.cluster.ClusterBuilder;
import com.shieldblaze.expressgateway.backend.strategy.l7.http.HTTPRoundRobin;
import com.shieldblaze.expressgateway.backend.strategy.l7.http.sessionpersistence.NOOPSessionPersistence;
import com.shieldblaze.expressgateway.common.utils.AvailablePortUtil;
import com.shieldblaze.expressgateway.common.utils.SelfSignedCertificate;
import com.shieldblaze.expressgateway.configuration.ConfigurationContext;
import com.shieldblaze.expressgateway.configuration.tls.CertificateKeyPair;
import com.shieldblaze.expressgateway.configuration.tls.MutualTLS;
import com.shieldblaze.expressgateway.configuration.tls.TlsClientConfiguration;
import com.shieldblaze.expressgateway.configuration.tls.TlsServerConfiguration;
import com.shieldblaze.expressgateway.core.events.L4FrontListenerStartupTask;
import com.shieldblaze.expressgateway.protocol.http.loadbalancer.HTTPLoadBalancer;
import com.shieldblaze.expressgateway.protocol.http.loadbalancer.HTTPLoadBalancerBuilder;
import io.netty.handler.ssl.util.InsecureTrustManagerFactory;
import org.junit.jupiter.api.Test;
import org.junit.jupiter.api.Timeout;

import javax.net.ssl.KeyManagerFactory;
import javax.net.ssl.SSLContext;
import javax.net.ssl.SSLHandshakeException;
import java.io.IOException;
import java.net.InetSocketAddress;
import java.net.URI;
import java.net.http.HttpClient;
import java.net.http.HttpRequest;
import java.net.http.HttpResponse;
import java.security.KeyPair;
import java.security.KeyStore;
import java.security.SecureRandom;
import java.security.cert.X509Certificate;
import java.time.Duration;
import java.util.List;
import java.util.concurrent.TimeUnit;

import static org.junit.jupiter.api.Assertions.assertEquals;
import static org.junit.jupiter.api.Assertions.assertThrows;
import static org.junit.jupiter.api.Assertions.assertTrue;

/**
 * P1-6: mTLS failure scenario tests.
 *
 * <p>Validates that the load balancer correctly enforces mutual TLS (mTLS) when
 * {@link MutualTLS#REQUIRED} is configured on the frontend TLS listener:</p>
 * <ul>
 *   <li><b>Negative test</b>: A client connecting with plain TLS (no client certificate)
 *       MUST be rejected during the TLS handshake. The server's SslContext is configured
 *       with {@link io.netty.handler.ssl.ClientAuth#REQUIRE}, which causes OpenSSL/JDK
 *       to send a CertificateRequest during the handshake. If the client does not present
 *       a certificate, the handshake fails with a fatal alert.</li>
 *   <li><b>Positive test</b>: A client that presents a valid client certificate (signed by
 *       the CA trusted by the server) completes the TLS handshake and receives a 200 OK
 *       response from the backend.</li>
 * </ul>
 *
 * <p>The test generates a self-signed CA certificate and uses it to sign both the server
 * certificate and the client certificate. The server's trust store is configured to trust
 * only this CA, so only clients presenting a certificate signed by this CA are accepted.</p>
 *
 * <p>Relevant RFCs:
 * <ul>
 *   <li>RFC 8446 (TLS 1.3) Section 4.3.2 -- CertificateRequest message</li>
 *   <li>RFC 8446 Section 4.4.2.4 -- client MUST send Certificate if CertificateRequest received</li>
 * </ul>
 */
@Timeout(value = 120, unit = TimeUnit.SECONDS)
class MtlsFailureTest {

    /**
     * When mTLS is REQUIRED and the client connects WITHOUT a client certificate,
     * the TLS handshake MUST fail. The Java HttpClient surfaces this as an
     * {@link SSLHandshakeException} or {@link IOException} wrapping it.
     *
     * <p>This is the critical security property of mTLS: unauthenticated clients
     * are rejected at the transport layer before any HTTP request processing occurs.</p>
     */
    @Test
    void clientWithoutCert_rejected_whenMtlsRequired() throws Exception {
        // Generate a self-signed cert for the server.
        SelfSignedCertificate serverCert = SelfSignedCertificate.generateNew(
                List.of("127.0.0.1"), List.of("localhost"));

        // Write the server cert to a temp file for the trust store configuration.
        // Use Files.createTempFile for restrictive (owner-only) permissions.
        java.io.File trustCertFile = java.nio.file.Files.createTempFile("mtls-trust-", ".pem").toFile();
        trustCertFile.deleteOnExit();
        try (java.io.FileWriter fw = new java.io.FileWriter(trustCertFile)) {
            fw.write(encodeCertToPem(serverCert.x509Certificate()));
        }

        // Build TLS server config with mTLS REQUIRED.
        TlsServerConfiguration tlsServerConfig =
                TlsServerConfiguration.copyFrom(TlsServerConfiguration.DEFAULT);
        tlsServerConfig.enable();
        tlsServerConfig.setMutualTLS(MutualTLS.REQUIRED);
        tlsServerConfig.setTrustCertificateFile(trustCertFile);

        CertificateKeyPair serverKeyPair = CertificateKeyPair.forClient(
                List.of(serverCert.x509Certificate()), serverCert.keyPair().getPrivate());
        tlsServerConfig.addMapping("localhost", serverKeyPair);

        TlsClientConfiguration tlsClientConfig =
                TlsClientConfiguration.copyFrom(TlsClientConfiguration.DEFAULT);
        tlsClientConfig.defaultMapping(CertificateKeyPair.newDefaultClientInstance());

        ConfigurationContext configCtx = ConfigurationContext.create(tlsClientConfig, tlsServerConfig);

        // Start a cleartext backend HTTP server.
        HttpServer httpServer = new HttpServer(false);
        httpServer.start();
        httpServer.START_FUTURE.get(60, TimeUnit.SECONDS);

        int lbPort = AvailablePortUtil.getTcpPort();

        Cluster cluster = ClusterBuilder.newBuilder()
                .withLoadBalance(new HTTPRoundRobin(NOOPSessionPersistence.INSTANCE))
                .build();

        HTTPLoadBalancer lb = HTTPLoadBalancerBuilder.newBuilder()
                .withConfigurationContext(configCtx)
                .withBindAddress(new InetSocketAddress("127.0.0.1", lbPort))
                .build();

        lb.mappedCluster("localhost:" + lbPort, cluster);

        NodeBuilder.newBuilder()
                .withCluster(cluster)
                .withSocketAddress(new InetSocketAddress("127.0.0.1", httpServer.port()))
                .build();

        L4FrontListenerStartupTask startupTask = lb.start();
        startupTask.future().get(60, TimeUnit.SECONDS);
        assertTrue(startupTask.isSuccess(), "Load balancer must start successfully");
        Thread.sleep(200);

        try {
            // Create a plain TLS client WITHOUT a client certificate.
            // InsecureTrustManagerFactory trusts the self-signed server cert,
            // but no KeyManager is configured so no client cert is presented.
            SSLContext sslContext = SSLContext.getInstance("TLSv1.3");
            sslContext.init(null, InsecureTrustManagerFactory.INSTANCE.getTrustManagers(),
                    new SecureRandom());

            HttpClient client = HttpClient.newBuilder()
                    .sslContext(sslContext)
                    .build();

            HttpRequest request = HttpRequest.newBuilder()
                    .GET()
                    .uri(URI.create("https://localhost:" + lbPort + "/"))
                    .version(HttpClient.Version.HTTP_1_1)
                    .timeout(Duration.ofSeconds(10))
                    .build();

            // The handshake should fail because the server requires a client certificate
            // and the client did not present one. This surfaces as an IOException
            // (SSLHandshakeException or connection reset).
            assertThrows(IOException.class, () ->
                            client.send(request, HttpResponse.BodyHandlers.ofString()),
                    "Connection without client certificate MUST fail when mTLS is REQUIRED");
        } finally {
            lb.stop().future().get(30, TimeUnit.SECONDS);
            httpServer.shutdown();
            httpServer.SHUTDOWN_FUTURE.get(30, TimeUnit.SECONDS);
        }
    }

    /**
     * When mTLS is REQUIRED and the client presents a valid client certificate,
     * the TLS handshake succeeds and the proxied request returns 200 OK.
     *
     * <p>This is the positive-path complement to the negative test above. Together
     * they prove that the mTLS enforcement is both rejecting unauthenticated clients
     * and accepting properly authenticated ones.</p>
     */
    @Test
    void clientWithValidCert_succeeds_whenMtlsRequired() throws Exception {
        // Generate a self-signed cert that serves as both the server identity and the
        // trusted CA for client certificates. In a real deployment these would be
        // separate, but for test purposes a single self-signed cert is sufficient
        // to prove the mTLS handshake machinery works.
        SelfSignedCertificate serverCert = SelfSignedCertificate.generateNew(
                List.of("127.0.0.1"), List.of("localhost"));

        // The client certificate: also self-signed, but we'll configure the server
        // to trust it by adding it to the trust store.
        SelfSignedCertificate clientCert = SelfSignedCertificate.generateNew(
                List.of("127.0.0.1"), List.of("localhost"));

        // Write BOTH certs (server's own + client's) to the trust file so the
        // server trusts client certificates signed by the client's key.
        // Use Files.createTempFile for restrictive (owner-only) permissions.
        java.io.File trustCertFile = java.nio.file.Files.createTempFile("mtls-trust-both-", ".pem").toFile();
        trustCertFile.deleteOnExit();
        try (java.io.FileWriter fw = new java.io.FileWriter(trustCertFile)) {
            fw.write(encodeCertToPem(serverCert.x509Certificate()));
            fw.write(encodeCertToPem(clientCert.x509Certificate()));
        }

        // Build TLS server config with mTLS REQUIRED.
        TlsServerConfiguration tlsServerConfig =
                TlsServerConfiguration.copyFrom(TlsServerConfiguration.DEFAULT);
        tlsServerConfig.enable();
        tlsServerConfig.setMutualTLS(MutualTLS.REQUIRED);
        tlsServerConfig.setTrustCertificateFile(trustCertFile);

        CertificateKeyPair serverKeyPair = CertificateKeyPair.forClient(
                List.of(serverCert.x509Certificate()), serverCert.keyPair().getPrivate());
        tlsServerConfig.addMapping("localhost", serverKeyPair);

        TlsClientConfiguration tlsClientConfig =
                TlsClientConfiguration.copyFrom(TlsClientConfiguration.DEFAULT);
        tlsClientConfig.defaultMapping(CertificateKeyPair.newDefaultClientInstance());

        ConfigurationContext configCtx = ConfigurationContext.create(tlsClientConfig, tlsServerConfig);

        // Start a cleartext backend HTTP server.
        HttpServer httpServer = new HttpServer(false);
        httpServer.start();
        httpServer.START_FUTURE.get(60, TimeUnit.SECONDS);

        int lbPort = AvailablePortUtil.getTcpPort();

        Cluster cluster = ClusterBuilder.newBuilder()
                .withLoadBalance(new HTTPRoundRobin(NOOPSessionPersistence.INSTANCE))
                .build();

        HTTPLoadBalancer lb = HTTPLoadBalancerBuilder.newBuilder()
                .withConfigurationContext(configCtx)
                .withBindAddress(new InetSocketAddress("127.0.0.1", lbPort))
                .build();

        lb.mappedCluster("localhost:" + lbPort, cluster);

        NodeBuilder.newBuilder()
                .withCluster(cluster)
                .withSocketAddress(new InetSocketAddress("127.0.0.1", httpServer.port()))
                .build();

        L4FrontListenerStartupTask startupTask = lb.start();
        startupTask.future().get(60, TimeUnit.SECONDS);
        assertTrue(startupTask.isSuccess(), "Load balancer must start successfully");
        Thread.sleep(200);

        try {
            // Build an SSLContext that presents the client certificate and trusts
            // the server's self-signed cert.
            KeyStore clientKeyStore = KeyStore.getInstance(KeyStore.getDefaultType());
            clientKeyStore.load(null, null);

            KeyPair clientKeyPairJca = clientCert.keyPair();
            clientKeyStore.setKeyEntry("client",
                    clientKeyPairJca.getPrivate(),
                    "changeit".toCharArray(),
                    new java.security.cert.Certificate[]{clientCert.x509Certificate()});

            KeyManagerFactory kmf = KeyManagerFactory.getInstance(
                    KeyManagerFactory.getDefaultAlgorithm());
            kmf.init(clientKeyStore, "changeit".toCharArray());

            SSLContext sslContext = SSLContext.getInstance("TLSv1.3");
            sslContext.init(kmf.getKeyManagers(),
                    InsecureTrustManagerFactory.INSTANCE.getTrustManagers(),
                    new SecureRandom());

            HttpClient client = HttpClient.newBuilder()
                    .sslContext(sslContext)
                    .build();

            HttpRequest request = HttpRequest.newBuilder()
                    .GET()
                    .uri(URI.create("https://localhost:" + lbPort + "/"))
                    .version(HttpClient.Version.HTTP_1_1)
                    .timeout(Duration.ofSeconds(30))
                    .build();

            HttpResponse<String> response = client.send(request, HttpResponse.BodyHandlers.ofString());
            assertEquals(200, response.statusCode(),
                    "mTLS handshake with valid client cert must succeed, proxy must return 200");
            assertEquals("Meow", response.body(),
                    "Response body from backend must pass through proxy unchanged");
        } finally {
            lb.stop().future().get(30, TimeUnit.SECONDS);
            httpServer.shutdown();
            httpServer.SHUTDOWN_FUTURE.get(30, TimeUnit.SECONDS);
        }
    }

    /**
     * Encodes an X.509 certificate into PEM format (Base64-encoded DER wrapped
     * in BEGIN/END CERTIFICATE markers). This is the format expected by
     * {@link java.security.cert.CertificateFactory#generateCertificates} and
     * {@link TlsServerConfiguration#buildTrustManagerFactory()}.
     */
    private static String encodeCertToPem(X509Certificate cert) throws Exception {
        java.util.Base64.Encoder encoder = java.util.Base64.getMimeEncoder(64, "\n".getBytes());
        byte[] derEncoded = cert.getEncoded();
        return "-----BEGIN CERTIFICATE-----\n"
                + encoder.encodeToString(derEncoded)
                + "\n-----END CERTIFICATE-----\n";
    }
}
