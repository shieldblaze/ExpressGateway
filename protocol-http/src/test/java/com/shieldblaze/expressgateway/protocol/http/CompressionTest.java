/*
 * This file is part of ShieldBlaze ExpressGateway. [www.shieldblaze.com]
 * Copyright (c) 2020 ShieldBlaze
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

import com.aayushatharva.brotli4j.decoder.DecoderJNI;
import com.aayushatharva.brotli4j.decoder.DirectDecompress;
import com.shieldblaze.expressgateway.backend.Node;
import com.shieldblaze.expressgateway.backend.cluster.Cluster;
import com.shieldblaze.expressgateway.backend.cluster.ClusterPool;
import com.shieldblaze.expressgateway.backend.strategy.l7.http.HTTPRoundRobin;
import com.shieldblaze.expressgateway.backend.strategy.l7.http.sessionpersistence.NOOPSessionPersistence;
import com.shieldblaze.expressgateway.concurrent.eventstream.EventStream;
import com.shieldblaze.expressgateway.configuration.CoreConfiguration;
import com.shieldblaze.expressgateway.configuration.CoreConfigurationBuilder;
import com.shieldblaze.expressgateway.configuration.BufferConfiguration;
import com.shieldblaze.expressgateway.configuration.EventLoopConfiguration;
import com.shieldblaze.expressgateway.configuration.HTTPConfiguration;
import com.shieldblaze.expressgateway.configuration.tls.CertificateKeyPair;
import com.shieldblaze.expressgateway.configuration.tls.Cipher;
import com.shieldblaze.expressgateway.configuration.tls.MutualTLS;
import com.shieldblaze.expressgateway.configuration.tls.Protocol;
import com.shieldblaze.expressgateway.configuration.tls.TLSClientConfiguration;
import com.shieldblaze.expressgateway.configuration.tls.TLSServerConfiguration;
import com.shieldblaze.expressgateway.configuration.transport.ReceiveBufferAllocationType;
import com.shieldblaze.expressgateway.configuration.transport.TransportConfiguration;
import com.shieldblaze.expressgateway.configuration.transport.TransportType;
import com.shieldblaze.expressgateway.core.events.L4FrontListenerStartupEvent;
import com.shieldblaze.expressgateway.core.events.L4FrontListenerStopEvent;
import com.shieldblaze.expressgateway.protocol.http.loadbalancer.HTTPLoadBalancer;
import com.shieldblaze.expressgateway.protocol.http.loadbalancer.HTTPLoadBalancerBuilder;
import com.shieldblaze.expressgateway.protocol.tcp.TCPListener;
import io.netty.buffer.Unpooled;
import io.netty.channel.ChannelHandler;
import io.netty.channel.ChannelHandlerContext;
import io.netty.channel.SimpleChannelInboundHandler;
import io.netty.handler.codec.http.DefaultFullHttpResponse;
import io.netty.handler.codec.http.FullHttpRequest;
import io.netty.handler.codec.http.HttpHeaderNames;
import io.netty.handler.codec.http.HttpResponseStatus;
import io.netty.handler.codec.http.HttpVersion;
import io.netty.handler.codec.http2.HttpConversionUtil;
import io.netty.handler.ssl.util.InsecureTrustManagerFactory;
import io.netty.handler.ssl.util.SelfSignedCertificate;
import org.junit.jupiter.api.BeforeAll;
import org.junit.jupiter.api.Test;

import javax.net.ssl.SSLContext;
import java.io.ByteArrayInputStream;
import java.io.IOException;
import java.net.InetSocketAddress;
import java.net.URI;
import java.net.http.HttpClient;
import java.net.http.HttpRequest;
import java.net.http.HttpResponse;
import java.security.SecureRandom;
import java.time.Duration;
import java.util.Collections;
import java.util.zip.DataFormatException;
import java.util.zip.GZIPInputStream;
import java.util.zip.Inflater;

import static org.junit.jupiter.api.Assertions.assertEquals;
import static org.junit.jupiter.api.Assertions.assertTrue;

class CompressionTest {

    static TransportConfiguration transportConfiguration;
    static EventLoopConfiguration eventLoopConfiguration;
    static CoreConfiguration coreConfiguration;
    static TLSServerConfiguration forServer;
    static TLSClientConfiguration forClient;
    static HTTPConfiguration httpConfiguration;
    static HttpClient httpClient;

    @BeforeAll
    static void initialize() throws Exception {
        transportConfiguration = new TransportConfiguration()
                .transportType(TransportType.NIO)
                .tcpFastOpenMaximumPendingRequests(2147483647)
                .backendConnectTimeout(1000 * 5)
                .receiveBufferAllocationType(ReceiveBufferAllocationType.FIXED)
                .receiveBufferSizes(new int[]{100})
                .socketReceiveBufferSize(2147483647)
                .socketSendBufferSize(2147483647)
                .tcpConnectionBacklog(2147483647)
                .connectionIdleTimeout(180);

        eventLoopConfiguration = new EventLoopConfiguration()
                .parentWorkers(2)
                .childWorkers(4);

        coreConfiguration = CoreConfigurationBuilder.newBuilder()
                .withTransportConfiguration(transportConfiguration)
                .withEventLoopConfiguration(eventLoopConfiguration)
                .withBufferConfiguration(BufferConfiguration.DEFAULT)
                .build();

        SelfSignedCertificate selfSignedCertificate = new SelfSignedCertificate("localhost", "EC", 256);

        CertificateKeyPair certificateKeyPair = new CertificateKeyPair(selfSignedCertificate.certificate().getAbsolutePath(),
                selfSignedCertificate.privateKey().getAbsolutePath(), false);

        forServer = new TLSServerConfiguration()
                .protocols(Collections.singletonList(Protocol.TLS_1_3))
                .ciphers(Collections.singletonList(Cipher.TLS_AES_128_GCM_SHA256))
                .mutualTLS(MutualTLS.NOT_REQUIRED);

        forServer.defaultMapping(certificateKeyPair);

        forClient = new TLSClientConfiguration()
                .protocols(Collections.singletonList(Protocol.TLS_1_3))
                .ciphers(Collections.singletonList(Cipher.TLS_AES_128_GCM_SHA256))
                .acceptAllCerts(true);

        httpConfiguration = new HTTPConfiguration()
                .brotliCompressionLevel(4)
                .compressionThreshold(100)
                .deflateCompressionLevel(6)
                .maxChunkSize(1024 * 100)
                .maxContentLength(1024 * 10240)
                .maxHeaderSize(1024 * 10)
                .maxInitialLineLength(1024 * 100)
                .h2InitialWindowSize(Integer.MAX_VALUE)
                .h2MaxConcurrentStreams(1000)
                .h2MaxHeaderSizeList(262144)
                .h2MaxFrameSize(16777215)
                .h2MaxHeaderTableSize(65536);

        SSLContext sslContext = SSLContext.getInstance("TLSv1.3");
        sslContext.init(null, InsecureTrustManagerFactory.INSTANCE.getTrustManagers(), new SecureRandom());

        httpClient = HttpClient.newBuilder()
                .sslContext(sslContext)
                .build();
    }

    @Test
    void brotliCompressionTest() throws InterruptedException, IOException {

        @ChannelHandler.Sharable
        class Handler extends SimpleChannelInboundHandler<FullHttpRequest> {
            @Override
            protected void channelRead0(ChannelHandlerContext ctx, FullHttpRequest msg) {
                DefaultFullHttpResponse httpResponse = new DefaultFullHttpResponse(HttpVersion.HTTP_1_1,
                        HttpResponseStatus.OK, Unpooled.wrappedBuffer("Meow".getBytes()));
                if (msg.headers().contains(HttpConversionUtil.ExtensionHeaderNames.STREAM_ID.text())) {
                    httpResponse.headers().set("x-http2-stream-id", msg.headers().get(HttpConversionUtil.ExtensionHeaderNames.STREAM_ID.text()));
                } else {
                    httpResponse.headers().set(HttpHeaderNames.CONTENT_LENGTH, 4);
                }
                httpResponse.headers().set(HttpHeaderNames.CONTENT_TYPE, "text/html");
                ctx.writeAndFlush(httpResponse);
            }
        }

        HTTPServer httpServer = new HTTPServer(10000, true, new Handler());
        httpServer.start();
        Thread.sleep(500L);

        Cluster cluster = new ClusterPool(new EventStream(), new HTTPRoundRobin(NOOPSessionPersistence.INSTANCE));
        new Node(cluster, new InetSocketAddress("127.0.0.1", 10000));

        HTTPLoadBalancer httpLoadBalancer = HTTPLoadBalancerBuilder.newBuilder()
                .withCoreConfiguration(coreConfiguration)
                .withHTTPConfiguration(httpConfiguration)
                .withTLSForClient(forClient)
                .withTLSForServer(forServer)
                .withCluster(cluster)
                .withBindAddress(new InetSocketAddress("127.0.0.1", 20000))
                .withHTTPInitializer(new DefaultHTTPServerInitializer())
                .withL4FrontListener(new TCPListener())
                .build();

        L4FrontListenerStartupEvent l4FrontListenerStartupEvent = httpLoadBalancer.start();
        l4FrontListenerStartupEvent.future().join();
        assertTrue(l4FrontListenerStartupEvent.success());

        // Brotli only
        {
            HttpRequest httpRequest = HttpRequest.newBuilder()
                    .GET()
                    .uri(URI.create("https://127.0.0.1:20000"))
                    .version(HttpClient.Version.HTTP_2)
                    .timeout(Duration.ofSeconds(5))
                    .setHeader("Accept-Encoding", "br")
                    .build();

            HttpResponse<byte[]> httpResponse = httpClient.send(httpRequest, HttpResponse.BodyHandlers.ofByteArray());
            assertEquals(200, httpResponse.statusCode());
            assertEquals("br", httpResponse.headers().firstValue("Content-Encoding").get());

            DirectDecompress directDecompress = DirectDecompress.decompress(httpResponse.body());
            assertEquals(DecoderJNI.Status.DONE, directDecompress.getResultStatus());
            assertEquals("Meow", new String(directDecompress.getDecompressedData()));
        }

        // Brotli and Gzip
        {
            HttpRequest httpRequest = HttpRequest.newBuilder()
                    .GET()
                    .uri(URI.create("https://127.0.0.1:20000"))
                    .version(HttpClient.Version.HTTP_2)
                    .timeout(Duration.ofSeconds(5))
                    .setHeader("Accept-Encoding", "gzip, br")
                    .build();

            HttpResponse<byte[]> httpResponse = httpClient.send(httpRequest, HttpResponse.BodyHandlers.ofByteArray());
            assertEquals(200, httpResponse.statusCode());
            assertEquals("br", httpResponse.headers().firstValue("Content-Encoding").get());

            DirectDecompress directDecompress = DirectDecompress.decompress(httpResponse.body());
            assertEquals(DecoderJNI.Status.DONE, directDecompress.getResultStatus());
            assertEquals("Meow", new String(directDecompress.getDecompressedData()));
        }

        // Brotli, Gzip and Deflate
        {
            HttpRequest httpRequest = HttpRequest.newBuilder()
                    .GET()
                    .uri(URI.create("https://127.0.0.1:20000"))
                    .version(HttpClient.Version.HTTP_2)
                    .timeout(Duration.ofSeconds(5))
                    .setHeader("Accept-Encoding", "gzip, deflate, br")
                    .build();

            HttpResponse<byte[]> httpResponse = httpClient.send(httpRequest, HttpResponse.BodyHandlers.ofByteArray());
            assertEquals(200, httpResponse.statusCode());
            assertEquals("br", httpResponse.headers().firstValue("Content-Encoding").get());

            DirectDecompress directDecompress = DirectDecompress.decompress(httpResponse.body());
            assertEquals(DecoderJNI.Status.DONE, directDecompress.getResultStatus());
            assertEquals("Meow", new String(directDecompress.getDecompressedData()));
        }

        // Shutdown HTTP Server and Load Balancer
        httpServer.shutdown();
        L4FrontListenerStopEvent l4FrontListenerStopEvent = httpLoadBalancer.stop();
        l4FrontListenerStopEvent.future().join();
        assertTrue(l4FrontListenerStopEvent.success());
    }

    @Test
    void gzipCompressionTest() throws InterruptedException, IOException {
        @ChannelHandler.Sharable
        class Handler extends SimpleChannelInboundHandler<FullHttpRequest> {
            @Override
            protected void channelRead0(ChannelHandlerContext ctx, FullHttpRequest msg) {
                DefaultFullHttpResponse httpResponse = new DefaultFullHttpResponse(HttpVersion.HTTP_1_1,
                        HttpResponseStatus.OK, Unpooled.wrappedBuffer("Meow".getBytes()));
                if (msg.headers().contains(HttpConversionUtil.ExtensionHeaderNames.STREAM_ID.text())) {
                    httpResponse.headers().set("x-http2-stream-id", msg.headers().get(HttpConversionUtil.ExtensionHeaderNames.STREAM_ID.text()));
                } else {
                    httpResponse.headers().set(HttpHeaderNames.CONTENT_LENGTH, 4);
                }
                httpResponse.headers().set(HttpHeaderNames.CONTENT_TYPE, "text/html");
                ctx.writeAndFlush(httpResponse);
            }
        }

        HTTPServer httpServer = new HTTPServer(10001, true, new Handler());
        httpServer.start();
        Thread.sleep(500L);

        Cluster cluster = new ClusterPool(new EventStream(), new HTTPRoundRobin(NOOPSessionPersistence.INSTANCE));
        new Node(cluster, new InetSocketAddress("127.0.0.1", 10001));

        HTTPLoadBalancer httpLoadBalancer = HTTPLoadBalancerBuilder.newBuilder()
                .withCoreConfiguration(coreConfiguration)
                .withHTTPConfiguration(httpConfiguration)
                .withTLSForClient(forClient)
                .withTLSForServer(forServer)
                .withCluster(cluster)
                .withBindAddress(new InetSocketAddress("127.0.0.1", 20001))
                .withHTTPInitializer(new DefaultHTTPServerInitializer())
                .withL4FrontListener(new TCPListener())
                .build();

        L4FrontListenerStartupEvent l4FrontListenerStartupEvent = httpLoadBalancer.start();
        l4FrontListenerStartupEvent.future().join();
        assertTrue(l4FrontListenerStartupEvent.success());

        // Gzip only
        {
            HttpRequest httpRequest = HttpRequest.newBuilder()
                    .GET()
                    .uri(URI.create("https://127.0.0.1:20001"))
                    .version(HttpClient.Version.HTTP_2)
                    .timeout(Duration.ofSeconds(5))
                    .setHeader("Accept-Encoding", "gzip")
                    .build();

            HttpResponse<byte[]> httpResponse = httpClient.send(httpRequest, HttpResponse.BodyHandlers.ofByteArray());
            assertEquals(200, httpResponse.statusCode());
            assertEquals("gzip", httpResponse.headers().firstValue("Content-Encoding").get());

            GZIPInputStream gzipInputStream = new GZIPInputStream(new ByteArrayInputStream(httpResponse.body()));
            assertEquals("Meow", new String(gzipInputStream.readAllBytes()));
            gzipInputStream.close();
        }

        // Gzip and Deflate
        {
            HttpRequest httpRequest = HttpRequest.newBuilder()
                    .GET()
                    .uri(URI.create("https://127.0.0.1:20001"))
                    .version(HttpClient.Version.HTTP_2)
                    .timeout(Duration.ofSeconds(5))
                    .setHeader("Accept-Encoding", "gzip, deflate")
                    .build();

            HttpResponse<byte[]> httpResponse = httpClient.send(httpRequest, HttpResponse.BodyHandlers.ofByteArray());
            assertEquals(200, httpResponse.statusCode());
            assertEquals("gzip", httpResponse.headers().firstValue("Content-Encoding").get());

            GZIPInputStream gzipInputStream = new GZIPInputStream(new ByteArrayInputStream(httpResponse.body()));
            assertEquals("Meow", new String(gzipInputStream.readAllBytes()));
            gzipInputStream.close();
        }

        // Shutdown HTTP Server and Load Balancer
        httpServer.shutdown();
        L4FrontListenerStopEvent l4FrontListenerStopEvent = httpLoadBalancer.stop();
        l4FrontListenerStopEvent.future().join();
        assertTrue(l4FrontListenerStopEvent.success());
    }

    @Test
    void deflateCompressionTest() throws InterruptedException, IOException, DataFormatException {
        @ChannelHandler.Sharable
        class Handler extends SimpleChannelInboundHandler<FullHttpRequest> {
            @Override
            protected void channelRead0(ChannelHandlerContext ctx, FullHttpRequest msg) {
                DefaultFullHttpResponse httpResponse = new DefaultFullHttpResponse(HttpVersion.HTTP_1_1,
                        HttpResponseStatus.OK, Unpooled.wrappedBuffer("Meow".getBytes()));
                if (msg.headers().contains(HttpConversionUtil.ExtensionHeaderNames.STREAM_ID.text())) {
                    httpResponse.headers().set("x-http2-stream-id", msg.headers().get(HttpConversionUtil.ExtensionHeaderNames.STREAM_ID.text()));
                } else {
                    httpResponse.headers().set(HttpHeaderNames.CONTENT_LENGTH, 4);
                }
                httpResponse.headers().set(HttpHeaderNames.CONTENT_TYPE, "text/html");
                ctx.writeAndFlush(httpResponse);
            }
        }

        HTTPServer httpServer = new HTTPServer(10002, true, new Handler());
        httpServer.start();
        Thread.sleep(500L);

        Cluster cluster = new ClusterPool(new EventStream(), new HTTPRoundRobin(NOOPSessionPersistence.INSTANCE));
        new Node(cluster, new InetSocketAddress("127.0.0.1", 10002));

        HTTPLoadBalancer httpLoadBalancer = HTTPLoadBalancerBuilder.newBuilder()
                .withCoreConfiguration(coreConfiguration)
                .withHTTPConfiguration(httpConfiguration)
                .withTLSForClient(forClient)
                .withTLSForServer(forServer)
                .withCluster(cluster)
                .withBindAddress(new InetSocketAddress("127.0.0.1", 20002))
                .withHTTPInitializer(new DefaultHTTPServerInitializer())
                .withL4FrontListener(new TCPListener())
                .build();

        L4FrontListenerStartupEvent l4FrontListenerStartupEvent = httpLoadBalancer.start();
        l4FrontListenerStartupEvent.future().join();
        assertTrue(l4FrontListenerStartupEvent.success());

        // Deflate only
        {
            HttpRequest httpRequest = HttpRequest.newBuilder()
                    .GET()
                    .uri(URI.create("https://127.0.0.1:20002"))
                    .version(HttpClient.Version.HTTP_2)
                    .timeout(Duration.ofSeconds(5))
                    .setHeader("Accept-Encoding", "deflate")
                    .build();

            HttpResponse<byte[]> httpResponse = httpClient.send(httpRequest, HttpResponse.BodyHandlers.ofByteArray());
            assertEquals(200, httpResponse.statusCode());
            assertEquals("deflate", httpResponse.headers().firstValue("Content-Encoding").get());

            Inflater inflater = new Inflater();
            inflater.setInput(httpResponse.body());
            byte[] result = new byte[1024];
            int length = inflater.inflate(result);
            inflater.end();
            assertEquals("Meow", new String(result, 0, length));
        }

        // Shutdown HTTP Server and Load Balancer
        httpServer.shutdown();
        L4FrontListenerStopEvent l4FrontListenerStopEvent = httpLoadBalancer.stop();
        l4FrontListenerStopEvent.future().join();
        assertTrue(l4FrontListenerStopEvent.success());
    }
}
