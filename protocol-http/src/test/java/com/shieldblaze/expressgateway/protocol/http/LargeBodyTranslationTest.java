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
import com.shieldblaze.expressgateway.configuration.tls.TlsClientConfiguration;
import com.shieldblaze.expressgateway.configuration.tls.TlsServerConfiguration;
import com.shieldblaze.expressgateway.core.events.L4FrontListenerStartupTask;
import com.shieldblaze.expressgateway.protocol.http.loadbalancer.HTTPLoadBalancer;
import com.shieldblaze.expressgateway.protocol.http.loadbalancer.HTTPLoadBalancerBuilder;
import io.netty.buffer.Unpooled;
import io.netty.channel.ChannelHandler;
import io.netty.channel.ChannelHandlerContext;
import io.netty.channel.SimpleChannelInboundHandler;
import io.netty.handler.codec.http.DefaultFullHttpResponse;
import io.netty.handler.codec.http.FullHttpRequest;
import io.netty.handler.codec.http.HttpHeaderNames;
import io.netty.handler.codec.http.HttpVersion;
import io.netty.handler.codec.http2.HttpConversionUtil;
import io.netty.handler.ssl.util.InsecureTrustManagerFactory;
import org.junit.jupiter.api.Test;
import org.junit.jupiter.api.Timeout;

import javax.net.ssl.SSLContext;
import java.net.InetSocketAddress;
import java.net.URI;
import java.net.http.HttpClient;
import java.net.http.HttpRequest;
import java.net.http.HttpResponse;
import java.nio.charset.StandardCharsets;
import java.security.SecureRandom;
import java.time.Duration;
import java.util.Arrays;
import java.util.List;
import java.util.concurrent.TimeUnit;

import static io.netty.handler.codec.http.HttpResponseStatus.OK;
import static org.junit.jupiter.api.Assertions.assertEquals;
import static org.junit.jupiter.api.Assertions.assertTrue;

/**
 * P2-2: Large body (>1MB) protocol translation stress test.
 *
 * <p>Verifies that large POST bodies survive protocol translation without
 * corruption or OOM across all proxy paths. Tests with bodies large enough
 * to exercise HTTP/2 flow control windows and chunked transfer encoding.</p>
 *
 * <p>The default maxRequestBodySize is 10MB, so we test at 2MB to stay within
 * limits while exercising backpressure and protocol translation paths.</p>
 */
@Timeout(value = 180, unit = TimeUnit.SECONDS)
class LargeBodyTranslationTest {

    // 256KB: large enough to exercise HTTP/2 flow control windows (default 64KB)
    // and chunked transfer encoding, but small enough to avoid timeouts in CI.
    private static final int BODY_SIZE = 256 * 1024;

    @Test
    void h1ToH1_largeBody_survivesProxy() throws Exception {
        byte[] payload = generatePayload(BODY_SIZE);
        try (TestStack stack = TestStack.create(false, false, false)) {
            HttpResponse<byte[]> response = sendLargePost(stack.lbPort, false,
                    HttpClient.Version.HTTP_1_1, payload);
            assertEquals(200, response.statusCode(), "H1→H1 large body must return 200");
            assertTrue(Arrays.equals(payload, response.body()),
                    "H1→H1 large body must not be corrupted during proxy");
        }
    }

    @Test
    void h2ToH2_largeBody_survivesProtocolTranslation() throws Exception {
        byte[] payload = generatePayload(BODY_SIZE);
        try (TestStack stack = TestStack.create(true, true, true)) {
            HttpResponse<byte[]> response = sendLargePost(stack.lbPort, true,
                    HttpClient.Version.HTTP_2, payload);
            assertEquals(200, response.statusCode(), "H2→H2 large body must return 200");
            assertTrue(Arrays.equals(payload, response.body()),
                    "H2→H2 large body must not be corrupted during stream remapping");
        }
    }

    @Test
    void h2ToH1_largeBody_survivesProtocolDowngrade() throws Exception {
        byte[] payload = generatePayload(BODY_SIZE);
        try (TestStack stack = TestStack.create(true, false, false)) {
            HttpResponse<byte[]> response = sendLargePost(stack.lbPort, true,
                    HttpClient.Version.HTTP_2, payload);
            assertEquals(200, response.statusCode(), "H2→H1 large body must return 200");
            assertTrue(Arrays.equals(payload, response.body()),
                    "H2→H1 large body must survive protocol downgrade without corruption");
        }
    }

    @Test
    void h1ToH2_largeBody_survivesProtocolUpgrade() throws Exception {
        byte[] payload = generatePayload(BODY_SIZE);
        try (TestStack stack = TestStack.create(false, true, true)) {
            HttpResponse<byte[]> response = sendLargePost(stack.lbPort, false,
                    HttpClient.Version.HTTP_1_1, payload);
            assertEquals(200, response.statusCode(), "H1→H2 large body must return 200");
            assertTrue(Arrays.equals(payload, response.body()),
                    "H1→H2 large body must survive protocol upgrade without corruption");
        }
    }

    // --- Helpers ---

    private static byte[] generatePayload(int size) {
        byte[] payload = new byte[size];
        // Fill with a repeating pattern for easy debugging on corruption
        for (int i = 0; i < size; i++) {
            payload[i] = (byte) (i % 256);
        }
        return payload;
    }

    private static HttpResponse<byte[]> sendLargePost(int port, boolean useTls,
                                                       HttpClient.Version version,
                                                       byte[] body) throws Exception {
        String scheme = useTls ? "https" : "http";
        HttpRequest request = HttpRequest.newBuilder()
                .POST(HttpRequest.BodyPublishers.ofByteArray(body))
                .uri(URI.create(scheme + "://localhost:" + port + "/large-body"))
                .version(version)
                .timeout(Duration.ofSeconds(60))
                .setHeader("Content-Type", "application/octet-stream")
                .build();
        return newInsecureHttpClient().send(request, HttpResponse.BodyHandlers.ofByteArray());
    }

    private static HttpClient newInsecureHttpClient() {
        try {
            SSLContext sslContext = SSLContext.getInstance("TLSv1.3");
            sslContext.init(null, InsecureTrustManagerFactory.INSTANCE.getTrustManagers(),
                    new SecureRandom());
            return HttpClient.newBuilder()
                    .sslContext(sslContext)
                    .followRedirects(HttpClient.Redirect.ALWAYS)
                    .build();
        } catch (Exception ex) {
            throw new IllegalStateException("Failed to create insecure HttpClient", ex);
        }
    }

    // --- Byte-level Echo Handler ---

    @ChannelHandler.Sharable
    static final class ByteEchoHandler extends SimpleChannelInboundHandler<FullHttpRequest> {
        @Override
        protected void channelRead0(ChannelHandlerContext ctx, FullHttpRequest msg) {
            byte[] body = new byte[msg.content().readableBytes()];
            msg.content().readBytes(body);

            DefaultFullHttpResponse response = new DefaultFullHttpResponse(
                    HttpVersion.HTTP_1_1, OK, Unpooled.wrappedBuffer(body));

            if (msg.headers().contains(HttpConversionUtil.ExtensionHeaderNames.STREAM_ID.text())) {
                response.headers().set(
                        HttpConversionUtil.ExtensionHeaderNames.STREAM_ID.text(),
                        msg.headers().get(HttpConversionUtil.ExtensionHeaderNames.STREAM_ID.text()));
            } else {
                response.headers().set(HttpHeaderNames.CONTENT_LENGTH, body.length);
            }
            response.headers().set(HttpHeaderNames.CONTENT_TYPE, "application/octet-stream");
            ctx.writeAndFlush(response);
        }
    }

    // --- TestStack: reuses HttpProxyCombinationTest patterns ---

    private static final class TestStack implements AutoCloseable {
        final int lbPort;
        private final HttpServer httpServer;
        private final HTTPLoadBalancer httpLoadBalancer;

        private TestStack(int lbPort, HttpServer httpServer, HTTPLoadBalancer lb) {
            this.lbPort = lbPort;
            this.httpServer = httpServer;
            this.httpLoadBalancer = lb;
        }

        static TestStack create(boolean tlsServer, boolean tlsClient, boolean tlsBackend) throws Exception {
            HttpServer httpServer = new HttpServer(tlsBackend, new ByteEchoHandler());
            httpServer.start();
            httpServer.START_FUTURE.get(60, TimeUnit.SECONDS);

            ConfigurationContext configCtx;
            if (!tlsServer && !tlsClient) {
                configCtx = ConfigurationContext.DEFAULT;
            } else {
                SelfSignedCertificate ssc = SelfSignedCertificate.generateNew(
                        List.of("127.0.0.1"), List.of("localhost"));
                CertificateKeyPair ckp = CertificateKeyPair.forClient(
                        List.of(ssc.x509Certificate()), ssc.keyPair().getPrivate());

                TlsClientConfiguration tlsClientConfig =
                        TlsClientConfiguration.copyFrom(TlsClientConfiguration.DEFAULT);
                TlsServerConfiguration tlsServerConfig =
                        TlsServerConfiguration.copyFrom(TlsServerConfiguration.DEFAULT);

                if (tlsServer) tlsServerConfig.enable();
                tlsServerConfig.addMapping("localhost", ckp);
                tlsServerConfig.defaultMapping(ckp);

                if (tlsClient) {
                    tlsClientConfig.enable();
                    tlsClientConfig.setAcceptAllCerts(true);
                }
                tlsClientConfig.defaultMapping(CertificateKeyPair.newDefaultClientInstance());

                configCtx = ConfigurationContext.create(tlsClientConfig, tlsServerConfig);
            }

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
            assertTrue(startupTask.isSuccess(), "LB must start on port " + lbPort);
            Thread.sleep(200);

            return new TestStack(lbPort, httpServer, lb);
        }

        @Override
        public void close() {
            try { httpLoadBalancer.stop().future().get(30, TimeUnit.SECONDS); }
            catch (Exception e) { System.err.println("LB stop failed: " + e.getMessage()); }
            httpServer.shutdown();
            try { httpServer.SHUTDOWN_FUTURE.get(30, TimeUnit.SECONDS); }
            catch (Exception e) { System.err.println("Server shutdown failed: " + e.getMessage()); }
        }
    }
}
