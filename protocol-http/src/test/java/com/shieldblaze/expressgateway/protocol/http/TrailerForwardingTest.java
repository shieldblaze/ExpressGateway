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
import com.shieldblaze.expressgateway.configuration.ConfigurationContext;
import com.shieldblaze.expressgateway.core.events.L4FrontListenerStartupTask;
import com.shieldblaze.expressgateway.protocol.http.loadbalancer.HTTPLoadBalancer;
import com.shieldblaze.expressgateway.protocol.http.loadbalancer.HTTPLoadBalancerBuilder;
import io.netty.buffer.Unpooled;
import io.netty.channel.ChannelHandler;
import io.netty.channel.ChannelHandlerContext;
import io.netty.channel.SimpleChannelInboundHandler;
import io.netty.handler.codec.http.DefaultHttpContent;
import io.netty.handler.codec.http.DefaultHttpResponse;
import io.netty.handler.codec.http.DefaultLastHttpContent;
import io.netty.handler.codec.http.FullHttpRequest;
import io.netty.handler.codec.http.HttpHeaderNames;
import io.netty.handler.codec.http.HttpHeaders;
import io.netty.handler.codec.http.HttpResponseStatus;
import io.netty.handler.codec.http.HttpVersion;
import io.netty.handler.codec.http.LastHttpContent;
import org.junit.jupiter.api.Test;
import org.junit.jupiter.api.Timeout;

import java.io.BufferedReader;
import java.io.Closeable;
import java.io.InputStreamReader;
import java.io.OutputStream;
import java.net.InetSocketAddress;
import java.net.Socket;
import java.nio.charset.StandardCharsets;
import java.util.concurrent.TimeUnit;

import static org.junit.jupiter.api.Assertions.assertEquals;
import static org.junit.jupiter.api.Assertions.assertNotNull;
import static org.junit.jupiter.api.Assertions.assertTrue;

/**
 * Tests that HTTP/1.1 trailers (RFC 9110 Section 6.5, RFC 9112 Section 7.1.2)
 * are correctly forwarded through the H1->H1 proxy path.
 *
 * <p>HTTP/1.1 chunked encoding allows trailing header fields after the final chunk.
 * A compliant proxy MUST forward these trailers to the client (unless they are
 * hop-by-hop headers, which are stripped). This test verifies that the
 * {@link DownstreamHandler} forwards trailers from the backend to the client.</p>
 *
 * <p>The test uses a custom backend handler that sends a chunked response with a
 * {@code Trailer: X-Checksum} header and an {@code X-Checksum} trailer in the
 * final chunk. A raw TCP client is used instead of Java's {@link java.net.http.HttpClient}
 * because the standard client does not expose HTTP/1.1 trailers.</p>
 *
 * <p>Wire format of the expected response (schematic):
 * <pre>
 * HTTP/1.1 200 OK\r\n
 * Transfer-Encoding: chunked\r\n
 * Trailer: X-Checksum\r\n
 * Content-Type: text/plain\r\n
 * \r\n
 * d\r\n
 * Hello, World!\r\n
 * 0\r\n
 * X-Checksum: abc123\r\n
 * \r\n
 * </pre>
 */
@Timeout(value = 60, unit = TimeUnit.SECONDS)
class TrailerForwardingTest {

    private static final String BODY_CONTENT = "Hello, World!";
    private static final String TRAILER_NAME = "X-Checksum";
    private static final String TRAILER_VALUE = "abc123";

    /**
     * Verifies the complete H1->H1 trailer forwarding path:
     * <ol>
     *   <li>Backend sends chunked response with {@code Trailer: X-Checksum} header</li>
     *   <li>Backend sends body chunk followed by a final chunk with X-Checksum trailer</li>
     *   <li>Proxy forwards the trailer to the client in the chunked response</li>
     * </ol>
     */
    @Test
    void h1ToH1_trailersForwarded() throws Exception {
        try (ProxyStack stack = ProxyStack.create()) {
            // Use a raw TCP socket to send HTTP/1.1 request and read the full
            // chunked response including trailers. Java's HttpClient does not
            // expose HTTP/1.1 trailers, so we must parse the wire format manually.
            try (Socket socket = new Socket("127.0.0.1", stack.lbPort)) {
                socket.setSoTimeout(10_000);

                // Send a minimal HTTP/1.1 GET request.
                OutputStream out = socket.getOutputStream();
                String request = "GET / HTTP/1.1\r\n" +
                        "Host: localhost:" + stack.lbPort + "\r\n" +
                        "Connection: close\r\n" +
                        "\r\n";
                out.write(request.getBytes(StandardCharsets.US_ASCII));
                out.flush();

                // Read the full response.
                try (BufferedReader reader = new BufferedReader(
                        new InputStreamReader(socket.getInputStream(), StandardCharsets.US_ASCII))) {

                    // --- Parse status line ---
                    String statusLine = reader.readLine();
                    assertNotNull(statusLine, "Status line must not be null");
                    assertTrue(statusLine.startsWith("HTTP/1.1 200"),
                            "Response must be 200 OK, got: " + statusLine);

                    // --- Parse headers ---
                    boolean hasTransferEncodingChunked = false;
                    boolean hasTrailerHeader = false;
                    String line;
                    while ((line = reader.readLine()) != null && !line.isEmpty()) {
                        String lower = line.toLowerCase();
                        if (lower.startsWith("transfer-encoding:") && lower.contains("chunked")) {
                            hasTransferEncodingChunked = true;
                        }
                        if (lower.startsWith("trailer:") && lower.contains(TRAILER_NAME.toLowerCase())) {
                            hasTrailerHeader = true;
                        }
                    }

                    assertTrue(hasTransferEncodingChunked,
                            "Response must use chunked Transfer-Encoding");
                    assertTrue(hasTrailerHeader,
                            "Response must declare Trailer: " + TRAILER_NAME + " in headers");

                    // --- Parse chunked body ---
                    // Read chunk size line.
                    String chunkSizeLine = reader.readLine();
                    assertNotNull(chunkSizeLine, "Chunk size line must not be null");
                    int chunkSize = Integer.parseInt(chunkSizeLine.trim(), 16);
                    assertTrue(chunkSize > 0, "First chunk must have non-zero size");

                    // Read chunk data.
                    char[] chunkData = new char[chunkSize];
                    int totalRead = 0;
                    while (totalRead < chunkSize) {
                        int read = reader.read(chunkData, totalRead, chunkSize - totalRead);
                        assertTrue(read > 0, "Must be able to read chunk data");
                        totalRead += read;
                    }
                    String bodyContent = new String(chunkData);
                    assertEquals(BODY_CONTENT, bodyContent,
                            "Body content must match what the backend sent");

                    // Read trailing CRLF after chunk data.
                    reader.readLine();

                    // --- Read final chunk (size = 0) ---
                    String finalChunkLine = reader.readLine();
                    assertNotNull(finalChunkLine, "Final chunk line must not be null");
                    assertEquals("0", finalChunkLine.trim(),
                            "Final chunk size must be 0");

                    // --- Parse trailers after final chunk ---
                    boolean foundTrailer = false;
                    String trailerLine;
                    while ((trailerLine = reader.readLine()) != null && !trailerLine.isEmpty()) {
                        if (trailerLine.toLowerCase().startsWith(TRAILER_NAME.toLowerCase() + ":")) {
                            String value = trailerLine.substring(trailerLine.indexOf(':') + 1).trim();
                            assertEquals(TRAILER_VALUE, value,
                                    "Trailer value must match what the backend sent");
                            foundTrailer = true;
                        }
                    }

                    assertTrue(foundTrailer,
                            "Response must contain the " + TRAILER_NAME + " trailer after the final chunk");
                }
            }
        }
    }

    // =========================================================================
    // Custom backend handler that sends a chunked response with trailers.
    //
    // Per RFC 9112 Section 7.1.2, a sender MUST NOT generate a trailer section
    // unless the sender knows the request includes a TE header containing
    // "trailers" OR the trailer fields are generated by a gateway. In this test,
    // the backend is a known test fixture, so we unconditionally send trailers.
    //
    // The Netty pipeline for cleartext H1 backend uses HttpServerCodec (no
    // HttpObjectAggregator), which supports streaming chunked responses with
    // trailers natively via HttpResponse + HttpContent + LastHttpContent.
    // =========================================================================

    @ChannelHandler.Sharable
    static final class ChunkedTrailerHandler extends SimpleChannelInboundHandler<FullHttpRequest> {

        @Override
        protected void channelRead0(ChannelHandlerContext ctx, FullHttpRequest msg) {
            // 1. Send the initial response headers (chunked, with Trailer declaration).
            DefaultHttpResponse response = new DefaultHttpResponse(
                    HttpVersion.HTTP_1_1, HttpResponseStatus.OK);
            response.headers().set(HttpHeaderNames.TRANSFER_ENCODING, "chunked");
            response.headers().set(HttpHeaderNames.CONTENT_TYPE, "text/plain");
            response.headers().set("Trailer", TRAILER_NAME);
            ctx.write(response);

            // 2. Send the body as one chunk.
            ctx.write(new DefaultHttpContent(
                    Unpooled.copiedBuffer(BODY_CONTENT, StandardCharsets.UTF_8)));

            // 3. Send the final chunk with trailing headers.
            LastHttpContent lastContent = new DefaultLastHttpContent(Unpooled.EMPTY_BUFFER);
            lastContent.trailingHeaders().set(TRAILER_NAME, TRAILER_VALUE);
            ctx.writeAndFlush(lastContent);
        }
    }

    // =========================================================================
    // ProxyStack: minimal H1->H1 proxy stack with the ChunkedTrailerHandler
    // backend. Matches the pattern from HttpProxyCombinationTest.
    // =========================================================================

    private static final class ProxyStack implements Closeable {

        final int lbPort;
        private final HttpServer httpServer;
        private final HTTPLoadBalancer httpLoadBalancer;

        private ProxyStack(int lbPort, HttpServer httpServer, HTTPLoadBalancer httpLoadBalancer) {
            this.lbPort = lbPort;
            this.httpServer = httpServer;
            this.httpLoadBalancer = httpLoadBalancer;
        }

        static ProxyStack create() throws Exception {
            // Start the backend HttpServer with our custom chunked-trailer handler.
            // useTls=false for H1->H1 cleartext path.
            HttpServer httpServer = new HttpServer(false, new ChunkedTrailerHandler());
            httpServer.start();
            httpServer.START_FUTURE.get(60, TimeUnit.SECONDS);

            // Build the load balancer on a dynamic port.
            int lbPort = AvailablePortUtil.getTcpPort();

            Cluster cluster = ClusterBuilder.newBuilder()
                    .withLoadBalance(new HTTPRoundRobin(NOOPSessionPersistence.INSTANCE))
                    .build();

            HTTPLoadBalancer httpLoadBalancer = HTTPLoadBalancerBuilder.newBuilder()
                    .withConfigurationContext(ConfigurationContext.DEFAULT)
                    .withBindAddress(new InetSocketAddress("127.0.0.1", lbPort))
                    .build();

            httpLoadBalancer.mappedCluster("localhost:" + lbPort, cluster);

            NodeBuilder.newBuilder()
                    .withCluster(cluster)
                    .withSocketAddress(new InetSocketAddress("127.0.0.1", httpServer.port()))
                    .build();

            L4FrontListenerStartupTask startupTask = httpLoadBalancer.start();
            startupTask.future().get(60, TimeUnit.SECONDS);
            assertTrue(startupTask.isSuccess(),
                    "Load balancer must start successfully on port " + lbPort);

            // Brief pause to ensure the server socket is fully ready.
            Thread.sleep(200);

            return new ProxyStack(lbPort, httpServer, httpLoadBalancer);
        }

        @Override
        public void close() {
            try {
                httpLoadBalancer.stop().future().get(30, TimeUnit.SECONDS);
            } catch (Exception e) {
                System.err.println("Warning: load balancer stop failed: " + e.getMessage());
            }

            httpServer.shutdown();
            try {
                httpServer.SHUTDOWN_FUTURE.get(30, TimeUnit.SECONDS);
            } catch (Exception e) {
                System.err.println("Warning: backend server shutdown failed: " + e.getMessage());
            }
        }
    }
}
